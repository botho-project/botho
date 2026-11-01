"""node_getStatus -> observation adapter: the x-series RPC half of the contract."""

import copy
import io
import json
from pathlib import Path
import tempfile
import unittest

from rpc_sustained_memory import fetch_status, main, observation_from_status
from sustained_memory import analyze
from test_sample_sustained_memory import SamplerTests
from test_sustained_memory import MIB, manifest, samples


def status(height, mining=True, transactions=0, **extra):
    return {
        "network": "botho-testnet",
        "gitCommit": "a" * 40,
        "chainHeight": height,
        "tipHash": f"{height:064x}",
        "mintingActive": mining,
        "totalTransactions": transactions,
        "peerCount": 4,
        **extra,
    }


def joined(plan, slope=0, mutate=None):
    """Synthetic memory rows whose chain fields come only from the adapter."""
    rows = samples(plan, slope)
    node = plan["nodes"][0]
    for index, row in enumerate(rows):
        reply = status(row["chain_height"], row["mining"], row["confirmed_transfers"])
        errors = ()
        if mutate:
            reply, errors = mutate(index, reply)
        observation = observation_from_status(
            plan,
            node,
            reply,
            row["observed_at_unix_s"] - 1,
            rpc_errors=errors,
            expected_network="botho-testnet",
        )
        for key in ("chain_height", "chain_hash", "confirmed_transfers", "mining"):
            row.pop(key, None)
            if key in observation:
                row[key] = observation[key]
        row["errors"] = observation["errors"]
    return rows


class AdapterTests(unittest.TestCase):
    def setUp(self):
        self.plan = manifest("minting")
        self.node = self.plan["nodes"][0]

    def adapt(self, reply, **kwargs):
        return observation_from_status(self.plan, self.node, reply, 5.0, **kwargs)

    def test_maps_one_atomic_reply_and_manifest_identity(self):
        observation = self.adapt(status(12, transactions=3))
        self.assertEqual(
            observation,
            {
                "observed_at_unix_s": 5.0,
                "pid": 42,
                "process_start_id": "boot:123",
                "chain_id": "isolated-control",
                "chain_height": 12,
                "chain_hash": f"{12:064x}",
                "mining": True,
                "confirmed_transfers": 3,
                "errors": [],
            },
        )

    def test_malformed_fields_are_omitted_and_reported_never_zeroed(self):
        for field, value in (
            ("chainHeight", None),
            ("chainHeight", -1),
            ("chainHeight", True),
            ("tipHash", "ABC"),
            ("tipHash", "G" * 64),
            ("mintingActive", 1),
            ("totalTransactions", "7"),
        ):
            with self.subTest(field=field, value=value):
                reply = status(12)
                reply[field] = value
                observation = self.adapt(reply)
                self.assertTrue(observation["errors"])
                if field in ("chainHeight", "tipHash"):
                    self.assertNotIn("chain_height", observation)
                    self.assertNotIn("chain_hash", observation)
        missing = status(12)
        del missing["mintingActive"]
        self.assertNotIn("mining", self.adapt(missing))

    def test_rpc_failure_keeps_identity_and_errors_only(self):
        observation = self.adapt(None, rpc_errors=["node_getStatus request failed"])
        self.assertEqual(observation["errors"], ["node_getStatus request failed"])
        self.assertNotIn("chain_height", observation)
        self.assertTrue(self.adapt("not an object")["errors"])

    def test_network_or_commit_contradiction_is_an_error(self):
        self.assertTrue(
            self.adapt(status(1), expected_network="botho-mainnet")["errors"]
        )
        self.assertTrue(self.adapt(status(1, gitCommit="b" * 40))["errors"])
        self.assertEqual(self.adapt(status(1, gitCommit="aaaaaaa"))["errors"], [])
        self.assertEqual(self.adapt(status(1, gitCommit="unknown"))["errors"], [])


class TimeSeriesThroughAdapterTests(unittest.TestCase):
    def setUp(self):
        self.plan = manifest("minting")

    def test_constant_and_plateau_pass(self):
        self.assertEqual(analyze(self.plan, joined(self.plan))["status"], "passed")
        rows = joined(self.plan)
        for row in rows:
            elapsed = row["observed_at_unix_s"] - self.plan["started_at_unix_s"]
            row["rss_bytes"] += int(min(elapsed / 1800, 1) * 30 * MIB)
        self.assertEqual(analyze(self.plan, rows)["status"], "passed")

    def test_old_49_mib_hour_retention_fails(self):
        result = analyze(self.plan, joined(self.plan, 49))
        self.assertEqual(result["status"], "failed")
        self.assertIn("n1: rss_plus_swap sustained growth exceeded", result["failures"])

    def test_rpc_outage_tick_is_incomplete_not_pass(self):
        def outage(index, reply):
            return (
                (None, ["node_getStatus request failed"])
                if index == 1000
                else (reply, ())
            )

        result = analyze(self.plan, joined(self.plan, mutate=outage))
        self.assertEqual(result["status"], "incomplete")

    def test_wrong_network_tick_is_incomplete(self):
        def drift(index, reply):
            if index == 1500:
                reply["network"] = "botho-mainnet"
            return reply, ()

        self.assertEqual(
            analyze(self.plan, joined(self.plan, mutate=drift))["status"], "incomplete"
        )

    def test_mining_never_paused_by_rpc_is_incomplete(self):
        def stuck(index, reply):
            reply["mintingActive"] = True
            return reply, ()

        self.assertEqual(
            analyze(self.plan, joined(self.plan, mutate=stuck))["status"], "incomplete"
        )

    def test_divergent_same_height_tip_fails(self):
        plan = manifest("minting")
        plan["nodes"].append(
            {
                **plan["nodes"][0],
                "node_id": "n2",
                "pid": 43,
                "cgroup_path": "/sys/fs/cgroup/lab-n2",
            }
        )
        first = joined(plan)
        second = []
        for row in copy.deepcopy(first):
            row.update(node_id="n2", pid=43, cgroup_path="/sys/fs/cgroup/lab-n2")
            second.append(row)
        second[400]["chain_hash"] = "f" * 64
        self.assertEqual(analyze(plan, first + second)["status"], "failed")


class SamplerJoinTests(SamplerTests):
    """The real sampler accepts adapter output unchanged."""

    def test_adapter_output_joins_sampler_row(self):
        self.observation = observation_from_status(
            self.plan, self.node, status(100), 999_998
        )
        row = self.collect()
        self.assertEqual(row["errors"], [])
        self.assertEqual(row["chain_height"], 100)
        self.assertIs(row["mining"], True)

    def test_adapter_errors_propagate_into_sampler_row(self):
        self.observation = observation_from_status(
            self.plan, self.node, None, 999_998, rpc_errors=["timeout"]
        )
        row = self.collect()
        self.assertIn("timeout", row["errors"])
        self.assertNotIn("chain_height", row)


class Response(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class CliTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.plan = manifest("minting")
        (self.root / "manifest.json").write_text(json.dumps(self.plan))
        self.out = self.root / "n1-observation.json"

    def run_cli(self, opener, url="http://127.0.0.1:7101/"):
        return main(
            [
                "--manifest",
                str(self.root / "manifest.json"),
                "--node",
                "n1",
                "--rpc-url",
                url,
                "--out",
                str(self.out),
                "--expected-network",
                "botho-testnet",
            ],
            now=lambda: 123.0,
            opener=opener,
        )

    def test_writes_observation_and_exit_zero(self):
        requests = []

        def opener(request, timeout, context):
            requests.append(json.loads(request.data))
            return Response(json.dumps({"result": status(9)}).encode())

        self.assertEqual(self.run_cli(opener), 0)
        self.assertEqual(requests[0]["method"], "node_getStatus")
        observation = json.loads(self.out.read_text())
        self.assertEqual(observation["observed_at_unix_s"], 123.0)
        self.assertEqual(observation["chain_height"], 9)

    def test_transport_and_rpc_errors_write_error_observation(self):
        def refused(request, timeout, context):
            raise ConnectionRefusedError("refused")

        self.assertEqual(self.run_cli(refused), 2)
        self.assertTrue(json.loads(self.out.read_text())["errors"])

        def rpc_error(request, timeout, context):
            return Response(b'{"error": {"code": -32603}}')

        self.assertEqual(self.run_cli(rpc_error), 2)
        self.assertNotIn("chain_height", json.loads(self.out.read_text()))

    def test_oversized_reply_is_rejected(self):
        def huge(request, timeout, context):
            return Response(b" " * (300 * 1024))

        result, errors = fetch_status("http://127.0.0.1/", 1, opener=huge)
        self.assertIsNone(result)
        self.assertTrue(errors)

    def test_credentialed_or_odd_urls_are_refused(self):
        for url in (
            "http://user:pw@127.0.0.1/",
            "ftp://127.0.0.1/rpc",
            "http://127.0.0.1/rpc?x=1",
        ):
            with self.subTest(url=url), self.assertRaises(SystemExit):
                self.run_cli(lambda *a, **k: None, url)
        self.assertFalse(self.out.exists())


if __name__ == "__main__":
    unittest.main()
