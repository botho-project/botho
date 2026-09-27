"""Linux proc/cgroup fixtures exercise the actual read-only sampler."""

import hashlib
from pathlib import Path
import tempfile
import unittest

from sample_sustained_memory import identity, sample
from test_sustained_memory import manifest


class SamplerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.proc = root / "proc"
        process = self.proc / "42"
        process.mkdir(parents=True)
        boot = self.proc / "sys/kernel/random"
        boot.mkdir(parents=True)
        (boot / "boot_id").write_text("boot\n")
        fields = ["0"] * 50
        fields[0], fields[11], fields[12], fields[19] = "R", "20", "10", "123"
        (process / "stat").write_text("42 (name with ) spaces) " + " ".join(fields))
        (process / "status").write_text(
            "VmRSS:\t2048 kB\nRssAnon:\t1024 kB\nVmSwap:\t512 kB\n"
        )
        (process / "exe").write_bytes(b"fixture binary")
        (process / "cgroup").write_text("0::/lab-node1\n")
        self.cgroups = root / "cgroups"
        group = self.cgroups / "lab-node1"
        group.mkdir(parents=True)
        for name, data in {
            "memory.current": "3000000",
            "memory.swap.current": "600000",
            "cpu.stat": "usage_usec 300000\nthrottled_usec 1500\n",
            "memory.events": "high 0\nmax 0\noom 0\noom_kill 0\n",
            **{
                f"{kind}.pressure": "some avg10=0.00 avg60=0.00 avg300=0.00 total=0"
                for kind in ("cpu", "memory", "io")
            },
        }.items():
            (group / name).write_text(data)
        self.machine = root / "machine-id"
        self.machine.write_text("h1\n")
        self.plan = manifest("minting")
        self.plan["artifact_sha256"] = hashlib.sha256(b"fixture binary").hexdigest()
        self.node = self.plan["nodes"][0]
        self.node["cgroup_path"] = str(group)
        self.observation = {
            "observed_at_unix_s": 999_998,
            "pid": 42,
            "process_start_id": "boot:123",
            "chain_id": self.node["chain_id"],
            "chain_height": 100,
            "chain_hash": "a" * 64,
            "confirmed_transfers": 0,
            "mining": True,
        }

    def collect(self):
        return sample(
            self.plan,
            self.node,
            self.observation,
            proc=self.proc,
            cgroups=self.cgroups,
            machine_id=self.machine,
            now=lambda: 1_000_000,
        )

    def test_reads_units_cpu_swap_and_binary_identity(self):
        row = self.collect()
        self.assertEqual(row["errors"], [])
        self.assertEqual(row["rss_bytes"], 2048 * 1024)
        self.assertEqual(row["swap_bytes"], 512 * 1024)
        self.assertEqual(row["cgroup_cpu_throttled_seconds"], 0.0015)
        self.assertEqual(row["artifact_sha256"], self.plan["artifact_sha256"])
        self.assertEqual(
            identity(42, self.proc, self.machine, self.cgroups)["process_start_id"],
            "boot:123",
        )

    def test_missing_metric_is_error_not_zero(self):
        (self.cgroups / "lab-node1/memory.swap.current").unlink()
        row = self.collect()
        self.assertTrue(row["errors"])
        self.assertNotIn("swap_bytes", row)

    def test_stale_or_wrong_process_rpc_is_error(self):
        self.observation["observed_at_unix_s"] = 1
        self.assertTrue(self.collect()["errors"])
        self.observation["observed_at_unix_s"] = 999_998
        self.observation["process_start_id"] = "boot:999"
        self.assertTrue(self.collect()["errors"])

    def test_sampler_records_actual_binary_change(self):
        (self.proc / "42/exe").write_bytes(b"different binary")
        self.assertNotEqual(
            self.collect()["artifact_sha256"], self.plan["artifact_sha256"]
        )


if __name__ == "__main__":
    unittest.main()
