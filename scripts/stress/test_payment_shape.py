"""x-series payment shape under the 10.0.0 fee floor (#2023). Offline only."""

import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import payment_shape as ps
from fee_evidence import RELAY_MIN_TX_FEE, floor_quote_summary, payment_consensus_summary, resign_fee
from plan import expand
from reporting import rehearsal_summary
from runtime import Gate, Journal, digest
import prefund
from selection import CONSOLIDATE_INPUTS, cover, ranked

HERE = Path(__file__).resolve().parent
PLAN = json.loads((HERE / "x-series-plan.json").read_text())
GOLDEN = ps.load_golden()


def plan(**prefund):
    p = copy.deepcopy(PLAN)
    p["prefund"].update(prefund)
    return p


def out(i, amount):
    return {"id": "%04d" % i, "utxo": {"amount": amount}}


class MeasuredFeesTests(unittest.TestCase):
    def setUp(self):
        self.plan = {**PLAN, "execution_mode": "rehearsal_only"}
        self.raw = json.dumps(self.plan, indent=2).encode()
        self.info = {"selected": [{"amount": 20_000_000_000}], "input_count": 1,
                     "fee_floor_quote": {"resignFee": 100_000_000, "minimumFee": 100_000_000,
                                         "settleFee": 100_000_000, "relayFeeFloor": 400_000,
                                         "consensusFeeFloor": 400_000}, "fee": 400_000_000,
                     "amount": 1_000_000_000}
        offers = len(ps.rehearsal_events(self.plan))
        self.rehearsal = {"plan_sha256": digest(self.plan), "status": "rehearsal_complete",
                          "qualification_status": "accepted_generator_limited",
                          "rehearsal": {"reconciled": offers,
                                        "payment_consensus_floors": payment_consensus_summary([self.info] * offers),
                                        "fee_floor_quotes": floor_quote_summary([self.info] * offers, per_input_value=True)}}
        batch_count = ps.batches(self.plan)
        self.funding = {"plan_sha256": hashlib.sha256(self.raw).hexdigest(), "batches": batch_count,
                        "completed_before_deadline": True,
                        "fee_floor_quotes": floor_quote_summary([self.info] * batch_count, per_input_value=False)}

    def test_report_counts_only_signed_rehearsal_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            journal = Journal(Path(tmp) / "journal.sqlite")
            self.addCleanup(journal.db.close)
            for identifier, kind, prepared in (("p", "rehearsal", 0), ("u", "rehearsal", None),
                                                ("c", "campaign", 1)):
                journal.offer(identifier, kind, 0, "wallet-1", "wallet-2", 1_000_000_000)
                if prepared is not None:
                    journal.transition(identifier, "prepared", prepared=prepared, info=self.info)
            summary = rehearsal_summary(journal, self.plan)
            self.assertEqual(summary["payment_consensus_floors"], payment_consensus_summary([self.info]))
            quotes = summary["fee_floor_quotes"]
            self.assertEqual(quotes, floor_quote_summary([self.info], per_input_value=True))
            journal.transition("u", "prepared", prepared=3, info={"fee": 400_000_000})
            self.assertEqual(rehearsal_summary(journal, self.plan)["fee_floor_quotes"],
                             {"signed": 2, "samples": 1, "p99": None})

    def test_exact_nearest_rank_for_payment_ratio_and_absolute_batch(self):
        # These ratios differ beyond double precision. Rank 99 of 100 must
        # take the second-largest ratio, even when the largest has a lower fee.
        n = 2**60
        rows = [dict(self.info, selected=[{"amount": n}, {"amount": i}], input_count=2,
                     fee_floor_quote={"resignFee": n}) for i in range(1, 100)]
        rows.append(dict(self.info, selected=[{"amount": n-1}], fee_floor_quote={"resignFee": n-1}))
        self.assertEqual(floor_quote_summary(rows, per_input_value=True)["p99"],
                         {"resign_fee_picocredits": n, "selected_input_picocredits": n+1})
        rows = [{"fee_floor_quote": {"resignFee": i}} for i in range(100, 0, -1)]
        self.assertEqual(floor_quote_summary(rows, per_input_value=False)["p99"], {"resign_fee_picocredits": 99})

    def test_incomplete_observations_and_historical_paid_fee_only(self):
        for bad in (dict(self.info, input_count=True), dict(self.info, selected=[]),
                    dict(self.info, selected=[{"amount": 0}]), dict(self.info, input_count=2),
                    dict(self.info, fee_floor_quote=None), {"fee": 800_000_000}):
            self.assertEqual(floor_quote_summary([self.info, bad], per_input_value=True),
                             {"signed": 2, "samples": 1, "p99": None})
        self.assertEqual(floor_quote_summary([], per_input_value=True), {"signed": 0, "samples": 0, "p99": None})

    def test_measured_floor_and_batch_are_compared_on_different_bases(self):
        result = ps.measured_fee_check(self.raw, self.rehearsal, self.funding)
        self.assertEqual(result["status"], "within_cap")
        self.assertEqual(result["shapes"][0]["required_picocredits"], 800_000_000)
        self.assertEqual(result["shapes"][0]["legacy_total_quote_projection_picocredits"], 3_200_000_000)
        self.assertEqual(result["shapes"][-1]["required_picocredits"], 100_000_000)
        # Exercise the exact decision boundary at tier 4 and a factor-2 margin.
        p99 = self.rehearsal["rehearsal"]["payment_consensus_floors"]["p99"]
        projected_value = result["shapes"][0]["moved_picocredits"]
        p99["output_value_picocredits"] = projected_value - 1
        self.rehearsal["rehearsal"]["payment_consensus_floors"]["max_selected_input_picocredits"] = projected_value
        cap = result["shapes"][0]["cap_picocredits"]
        base = result["shapes"][0]["base_ceiling_picocredits"]
        low, high = 0, cap
        while low + 1 < high:
            mid = (low + high) // 2
            if 8 * resign_fee(mid) <= cap:
                low = mid
            else:
                high = mid
        p99["consensus_floor_picocredits"] = low - base - 2
        self.assertEqual(ps.measured_fee_check(self.raw, self.rehearsal, self.funding)["status"], "within_cap")
        p99["consensus_floor_picocredits"] += 1
        self.assertEqual(ps.measured_fee_check(self.raw, self.rehearsal, self.funding)["status"], "over_cap")

    def test_missing_raw_or_inconsistent_settling_quote_stays_incomplete(self):
        p99 = payment_consensus_summary([self.info])["p99"]
        self.assertEqual(p99, {"consensus_floor_picocredits": 400_000, "output_value_picocredits": 19_600_000_000})
        for changed in ({"relayFeeFloor": True}, {"relayFeeFloor": 500}, {"settleFee": 99_999_999},
                        {"resignFee": 99_999_999}, {"minimumFee": None}):
            bad = {**self.info, "fee_floor_quote": {**self.info["fee_floor_quote"], **changed}}
            self.assertEqual(payment_consensus_summary([self.info, bad]), {"signed": 2, "samples": 1, "p99": None,
                                                     "max_selected_input_picocredits": 20_000_000_000})
        for bad in (dict(self.info, fee=20_000_000_000), dict(self.info, fee=99_999_999),
                    dict(self.info, outputs=[{"amount": 1}]),
                    dict(self.info, selected=[{"amount": 20_000_000_000}] * 5, input_count=5)):
            self.assertIsNone(payment_consensus_summary([bad])["p99"])
        self.rehearsal["rehearsal"]["payment_consensus_floors"]["max_selected_input_picocredits"] = 81_000_000_000
        row = ps.measured_fee_check(self.raw, self.rehearsal, self.funding)["shapes"][0]
        self.assertEqual(row["moved_picocredits"], 81_000_000_000)
        del self.rehearsal["rehearsal"]["payment_consensus_floors"]
        self.assertEqual(ps.measured_fee_check(self.raw, self.rehearsal, self.funding)["status"], "incomplete")

    def test_consensus_floor_p99_orders_by_actual_output_value(self):
        # One outlier has the same input total and raw floor but less change.
        low = dict(self.info, fee=200_000_000)
        high = dict(self.info, fee=800_000_000)
        typical = dict(self.info, outputs=[{"amount": 1_000_000_000}, {"amount": 18_600_000_000}])
        result = payment_consensus_summary([high, typical] + [low] * 98)
        self.assertEqual(result["samples"], 100)
        self.assertEqual(result["p99"], {"consensus_floor_picocredits": 400_000,
                                          "output_value_picocredits": 19_600_000_000})

    def test_rust_wallet_envelope_and_projection_cover_direct_prices(self):
        self.assertEqual(RELAY_MIN_TX_FEE, GOLDEN["relay_min_tx_fee"])
        for row in GOLDEN["rows"]:
            self.assertEqual(resign_fee(row["relay_floor_max"]), row["resign_fee"])
        self.assertEqual(resign_fee(2**64 - 1), 2**64 - 1)
        base = max(GOLDEN["payment_base_ceiling_by_input_count"])
        payments = [r for r in GOLDEN["rows"] if r["outputs"] == 2]
        target = 80_000_000_000
        direct = {}
        for r in payments:
            if r["moved"] == target:
                key = (r["wealth_bth"], r["depth"])
                direct[key] = max(direct.get(key, 0), r["resign_fee"])
        cap = int(PLAN["limits"]["max_single_signed_fee_picocredits"])
        verdicts = set()
        for r in payments:
            bound = resign_fee(ps.projected_relay_floor(r["consensus_floor"], r["moved"], target, base))
            actual = direct[r["wealth_bth"], r["depth"]]
            self.assertGreaterEqual(bound, actual, r)
            verdicts.add((actual * 8 <= cap, bound * 8 <= cap))
        self.assertIn((True, True), verdicts)
        self.assertIn((False, False), verdicts)
        self.assertEqual(verdicts, {(False, False), (True, True)})

    def test_campaign_requires_matching_payloads_and_economic_settings(self):
        candidate = {**PLAN, "endpoints": list(PLAN["endpoints"]), "genesis": "a" * 64}
        pin = {"commit": candidate["node_commit"], "node_sha256": "b" * 64, "signer_sha256": "c" * 64}
        self.rehearsal.update(node_sha256=pin["node_sha256"], signer_sha256=pin["signer_sha256"])
        def check(c, p):
            return ps.campaign_fee_check(self.raw, self.rehearsal, self.funding, json.dumps(c).encode(), p)
        good = check(candidate, pin)
        self.assertEqual(good["status"], "within_cap")
        self.assertEqual(good["campaign"]["replayed_payments_including_rehearsal"], 22240)
        for key in ("commit", "node_sha256", "signer_sha256"):
            with self.assertRaisesRegex(ValueError, "campaign .*differs"):
                check(candidate, {**pin, key: "d" * len(pin[key])})
        with self.assertRaisesRegex(ValueError, "differs from rehearsal: prefund"):
            check({**candidate, "prefund": {**candidate["prefund"], "grant_picocredits": "16000000000"}}, pin)
        with self.assertRaisesRegex(ValueError, "payment cap differs"):
            check({**candidate, "limits": {**candidate["limits"], "max_single_signed_fee_picocredits": "2000000000"}}, pin)
        with self.assertRaisesRegex(ValueError, "72-hour campaign"):
            check(self.plan, pin)

    def test_only_finished_and_matching_reports_support_the_comparison(self):
        self.assertEqual(ps.measured_fee_check(self.raw, funding=self.funding)["status"], "incomplete")
        self.rehearsal["status"] = "held"
        self.assertEqual(ps.measured_fee_check(self.raw, self.rehearsal, self.funding)["status"], "incomplete")
        self.rehearsal["status"] = "rehearsal_complete"
        self.rehearsal["rehearsal"]["reconciled"] = 2
        with self.assertRaisesRegex(ValueError, "counts differ"):
            ps.measured_fee_check(self.raw, self.rehearsal, self.funding)
        with self.assertRaisesRegex(ValueError, "funding health plan digest"):
            ps.measured_fee_check(self.raw + b" ", funding=self.funding)
        self.rehearsal["plan_sha256"] = self.funding["plan_sha256"]
        with self.assertRaisesRegex(ValueError, "rehearsal report plan digest"):
            ps.measured_fee_check(self.raw, self.rehearsal)

    def staging_profile(self):
        import staging
        from test_staging import StagingTests

        fixture = StagingTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        profile = json.loads(json.dumps(fixture.p).replace("campaign-test", "q20261118-x2-endurance-a"))
        candidate = dict(PLAN, endpoints=[t["rpc_url"] for t in profile["targets"]], genesis="e" * 64)
        Path(profile["plan_file"]).write_text(json.dumps(candidate, indent=2) + "\n")
        profile["isolated_discovery"] = True
        profile["opening_balances"] = [800_000_000_000] * 64
        for index in range(len(profile["wallets"]), 64):
            key = fixture.root / f"wallet-{index}"
            key.write_text(f"test wallet {index}")
            key.chmod(0o600)
            profile["wallets"].append({"address": str(index), "key": str(key)})
        self.rehearsal.update(run_id="separate-rehearsal-a", node_sha256=profile["node_sha256"],
                              signer_sha256=hashlib.sha256(Path(profile["signer"]).read_bytes()).hexdigest())
        inputs = [fixture.root / name for name in ("part-a.json", "report.json", "funding.json")]
        for path, raw in zip(inputs, (self.raw, json.dumps(self.rehearsal).encode(), json.dumps(self.funding).encode())):
            path.write_bytes(raw)
        profile["part_a_fee_evidence"] = staging.part_a_fee_sources(profile["plan_file"], *inputs)
        return fixture, profile, inputs

    def test_stages_exact_campaign_bytes_and_recomputed_fee_receipt(self):
        import staging
        import tarfile

        fixture, profile, inputs = self.staging_profile()
        fixture.profile.write_text(json.dumps(profile))
        output = fixture.root / "staged"
        with patch("staging.subprocess.run") as remote:
            staging.stage(fixture.profile, output)
        remote.assert_not_called()
        self.assertEqual(len(list(output.glob("*.tar"))), 6)
        with tarfile.open(next(output.glob("*-0.tar"))) as archive:
            plan_raw = archive.extractfile("state/plan.json").read()
            receipt = json.load(archive.extractfile("state/part-a-fees.json"))
        self.assertEqual(plan_raw, Path(profile["plan_file"]).read_bytes())
        self.assertEqual(receipt["campaign"]["plan_file_sha256"], hashlib.sha256(plan_raw).hexdigest())
        self.assertEqual(receipt["part_a_input_sha256"],
                         {k: hashlib.sha256(p.read_bytes()).hexdigest() for k, p in zip(("plan", "report", "funding"), inputs)})
        self.assertEqual(receipt["status"], "within_cap")
        self.assertEqual(receipt["campaign"]["replayed_payments_including_rehearsal"], 22240)

    def test_campaign_stage_stops_before_ssh_or_archiving_for_invalid_fees(self):
        import staging

        fixture, profile, inputs = self.staging_profile()
        output = fixture.root / "staged"
        def refuses(candidate, message):
            fixture.profile.write_text(json.dumps(candidate))
            with patch("staging.subprocess.run") as remote, self.assertRaisesRegex((Gate, ValueError), message):
                staging.stage(fixture.profile, output, execute=True)
            remote.assert_not_called()
            self.assertFalse(output.exists())
        refuses({k: v for k, v in profile.items() if k != "part_a_fee_evidence"}, "requires Part A")
        refuses({**profile, "node_sha256": "1" * 64}, "node_sha256")
        plan_path = Path(profile["plan_file"])
        raw = plan_path.read_bytes()
        plan_path.write_bytes(raw + b" ")
        refuses(profile, "campaign plan differs")
        plan_path.write_bytes(raw)
        signer = Path(profile["signer"])
        raw = signer.read_bytes()
        signer.write_bytes(raw + b" ")
        refuses(profile, "signer_sha256")
        signer.write_bytes(raw)
        for key, path in zip(("plan", "report", "funding"), inputs):
            raw = path.read_bytes()
            path.write_bytes(raw + b" ")
            refuses(profile, "input changed: " + key)
            path.write_bytes(raw)
        for status in ("incomplete", "over_cap"):
            amended = copy.deepcopy(self.rehearsal)
            if status == "incomplete":
                amended["status"] = "held"
            else:
                amended["rehearsal"]["payment_consensus_floors"]["p99"]["consensus_floor_picocredits"] = 10**9
            inputs[1].write_text(json.dumps(amended))
            profile["part_a_fee_evidence"] = staging.part_a_fee_sources(profile["plan_file"], *inputs)
            refuses(profile, "Part A fee decision: " + status)


class GoldenTests(unittest.TestCase):
    def test_golden_is_the_cargo_generated_grid(self):
        # Generated by cluster-tax/tests/stress_fee_shape.rs, never by hand.
        self.assertIn("stress_fee_shape", GOLDEN["generator"])
        self.assertEqual(GOLDEN["coinbase_picocredits"], 50 * 10**12)
        rows = GOLDEN["rows"]
        self.assertEqual({r["depth"] for r in rows}, {"hop", "exit", "ring"})
        # Ring rows (#2023 review) exist for prefund batches only.
        self.assertEqual({r["outputs"] for r in rows if r["depth"] == "ring"}, {16})
        self.assertTrue(all(r["consensus_floor"] <= r["relay_floor_max"] <= r["resign_fee"]
                            for r in rows))
        # The review's table (#2019): a full exit of 1 BTH from a 1M BTH
        # cluster owes ~8.2e10, one 5% hop ~8.2e8.
        def at(moved, wealth, depth):
            return next(r for r in rows if (r["inputs"], r["outputs"], r["moved"],
                        r["wealth_bth"], r["depth"]) == (1, 2, moved, wealth, depth))
        self.assertEqual(at(10**12, 1_000_000, "exit")["consensus_floor"] // 10**8, 818)
        self.assertEqual(at(10**12, 1_000_000, "hop")["consensus_floor"] // 10**7, 82)

    def test_shape_is_priced_at_the_next_larger_row(self):
        rows = ps.price(GOLDEN, 1, 2, 2 * 10**10 + 1)
        self.assertEqual({r["moved"] for r in rows}, {3 * 10**10})
        with self.assertRaisesRegex(ValueError, "exceeds the golden grid"):
            ps.price(GOLDEN, 1, 2, 10**15)
        with self.assertRaisesRegex(ValueError, "no golden rows"):
            ps.price(GOLDEN, 5, 2, 10**10)


class FeeCheckTests(unittest.TestCase):
    def test_x_series_plan_passes_every_tier_with_margin(self):
        report = ps.fee_check(PLAN)
        gated = [e for e in report if e["depth"] == "hop"]
        self.assertEqual({e["tier"] for e in gated if e["shape"] != "prefund batch"}, {1, 2, 4})
        self.assertTrue(all(e["fee"] * ps.FEE_MARGIN <= e["cap"] for e in gated))
        worst = {(s["shape"], s["tier"]): s for s in ps.summarize(report)}
        # One-input payments (up to a four-input probe's change, 8e10) pay
        # the relay minimum at a hop, times the tier, and survive a full exit
        # from a 200 BTH cluster at tier 4.
        self.assertEqual(worst[("payment, one input", 4)]["moved"], 8 * 10**10)
        self.assertEqual(worst[("payment, one input", 4)]["hop_fee"], 4 * 10**8)
        self.assertGreaterEqual(worst[("payment, one input", 4)]["exit_fits_cap_to_wealth_bth"], 200)
        # The batch is gated at its ring downgrade up to the prefunder's
        # cluster-wealth bound, 50 x (1 + 0.95 + 0.95^2) BTH, priced at 150.
        batch = worst[("prefund batch", 1)]
        self.assertAlmostEqual(batch["ring_bound_bth"], 142.625)
        self.assertEqual(batch["ring_wealth_bth"], 150)
        self.assertLessEqual(batch["ring_fee"] * ps.FEE_MARGIN, batch["cap"])

    def test_ring_rows_past_the_bound_are_reported_not_gated(self):
        report = ps.fee_check(PLAN)
        ring = [e for e in report if e["depth"] == "ring"]
        self.assertTrue(ring and all(e["shape"] == "prefund batch" for e in ring))
        self.assertEqual({e["wealth_bth"] for e in ring if e["gated"]}, {50, 150})
        # At 1k BTH (unbounded change chaining) the batch would owe ~2.4e11.
        self.assertTrue(any(e["fee"] > e["cap"] for e in ring if not e["gated"]))

    def test_deeper_funding_chains_fail_the_batch_ring_gate(self):
        # Ten hops: ~431 BTH, priced at the 1k BTH row, over the 1e11 cap.
        with patch.object(prefund, "MAX_FUNDING_DEPTH", 10):
            self.assertGreater(ps.ring_bound_bth(GOLDEN), 400)
            with self.assertRaisesRegex(ValueError, '"depth": "ring"'):
                ps.fee_check(PLAN)

    def test_whole_bth_grants_fail_at_the_upper_tiers(self):
        # The pre-#2023 shape: every payment moves a whole 1 BTH grant.
        with self.assertRaisesRegex(ValueError, "payment, one input"):
            ps.fee_check(plan(grant_picocredits=str(10**12)))

    def test_a_prefund_batch_cannot_live_under_max_single(self):
        # One 50 BTH coinbase moved at a 5% hop already owes ~1.06e9 to re-sign.
        with self.assertRaisesRegex(ValueError, "prefund batch"):
            ps.fee_check(plan(max_batch_fee_picocredits=PLAN["limits"][
                "max_single_signed_fee_picocredits"]))

    def test_campaign_budget_arithmetic_still_holds(self):
        summary, events = expand(copy.deepcopy(PLAN))
        attempts = len(events) + summary["rehearsal_payments"]
        self.assertLessEqual(attempts * int(PLAN["limits"]["max_single_signed_fee_picocredits"]),
                             int(PLAN["limits"]["max_signed_fee_picocredits"]))


class PrefundCheckTests(unittest.TestCase):
    def test_x_series_prefund(self):
        report = ps.prefund_check(PLAN)
        self.assertEqual(report["batches"], 171)  # ceil(64 * 40 / 15)
        self.assertEqual(report["principal_picocredits"], 64 * 40 * 2 * 10**10)
        self.assertLessEqual(report["fresh_chain_seconds"],
                             ps.PREFUND_WINDOW_SHARE * report["prefund_window_seconds"])
        self.assertLessEqual(report["worst_case_batch_fees"],
                             int(PLAN["limits"]["max_signed_fee_picocredits"]))

    def test_refusals(self):
        with self.assertRaisesRegex(ValueError, "prefund window"):
            ps.prefund_check(plan(grants_per_wallet=100))
        with self.assertRaisesRegex(ValueError, "largest transfer"):
            ps.prefund_check(plan(grant_picocredits=str(10**10)))
        with self.assertRaisesRegex(ValueError, "min_mature_utxos_per_wallet"):
            ps.prefund_check(plan(grants_per_wallet=15))
        with self.assertRaisesRegex(ValueError, "program bound"):
            ps.prefund_check(plan(max_batch_fee_picocredits=str(10**12 + 1)))


class CoverageTests(unittest.TestCase):
    def test_x_series_workload_is_covered(self):
        replay = ps.coverage(PLAN)
        self.assertEqual(replay["uncovered"], 0)
        self.assertEqual(replay["payments"], 21880 + 360)
        projection = replay["rehearsal_relay_minimum_projection"]
        self.assertEqual(projection["covered_rehearsal_offers"], 360)
        self.assertEqual(projection["quote_picocredits"], min(r["resign_fee"] for r in GOLDEN["rows"]))
        self.assertEqual(projection["minimum_projected_fee_picocredits"], 3_200_000_000)
        self.assertTrue(projection["exceeds_cap_at_relay_minimum"])
        # The four-input probes move the most: four whole grants. Their change
        # (up to 7.68e10) is later spent as one input: the free-shape bound is
        # the probe's, not 4 x need (#2023 review).
        self.assertEqual(replay["largest_value_moved"], 8 * 10**10)
        self.assertGreater(replay["largest_value_moved_by_inputs"]["1"], 4 * 11 * 10**9)
        for inputs, value in replay["largest_value_moved_by_inputs"].items():
            self.assertLessEqual(value, replay["priced_value_moved_by_inputs"][inputs])
        self.assertLessEqual(replay["worst_gated_fee"],
                             int(PLAN["limits"]["max_single_signed_fee_picocredits"]))

    def test_a_replayed_fee_over_the_cap_fails_never_clips(self):
        golden = copy.deepcopy(GOLDEN)
        for row in golden["rows"]:
            if (row["inputs"], row["outputs"]) == (1, 2):
                row["resign_fee"] *= 10
        with self.assertRaisesRegex(ValueError, "gated fee .* exceeds max_single"):
            ps.coverage(PLAN, golden=golden)

    def test_a_replayed_spend_past_the_priced_shapes_fails(self):
        # Priced as if no spend moved more than 4e10: the probes' 8e10 fails.
        original = ps.planned_shapes

        def shapes(plan, golden):
            return [(n, i, o, min(m, 4 * 10**10), t, c, r)
                    for n, i, o, m, t, c, r in original(plan, golden)]
        with patch.object(ps, "planned_shapes", shapes):
            with self.assertRaisesRegex(ValueError, "moves more than any priced shape"):
                ps.coverage(PLAN)

    def test_too_few_grants_leave_payments_uncovered(self):
        replay = ps.coverage(plan(grants_per_wallet=16))
        self.assertGreater(replay["uncovered"], 0)


class CoverTests(unittest.TestCase):
    NEED = 11 * 10**9

    def test_smallest_sufficient_single(self):
        outputs = [out(1, 5 * 10**10), out(2, 2 * 10**10), out(3, 12 * 10**9), out(4, 10**9)]
        self.assertEqual(cover(outputs, self.NEED)[0], [outputs[2]])
        self.assertEqual(cover(outputs, self.NEED, 1)[0], [outputs[2]])

    def test_ties_break_by_id(self):
        outputs = [out(2, 2 * 10**10), out(1, 2 * 10**10)]
        self.assertEqual(cover(outputs, self.NEED)[0], [outputs[1]])

    def test_least_value_combination_of_the_fewest_inputs(self):
        outputs = [out(i, a) for i, a in enumerate((9, 8, 3, 2, 1))]
        outputs = [{**o, "utxo": {"amount": o["utxo"]["amount"] * 10**9}} for o in outputs]
        selected, _ = cover(outputs, self.NEED)
        # Two inputs of exactly 1.1e10, (9, 2) before (8, 3) by ids.
        self.assertEqual([o["id"] for o in selected], ["0000", "0003"])
        selected, _ = cover(outputs, self.NEED, 4)
        self.assertEqual(sorted(o["utxo"]["amount"] for o in selected),
                         [10**9, 2 * 10**9, 3 * 10**9, 8 * 10**9])

    def test_padding_consolidates_dust_within_its_bound(self):
        dust = [out(10 + i, 10**9) for i in range(5)]
        grant = out(1, 2 * 10**10)
        selected, _ = cover([grant] + dust, self.NEED, pad=CONSOLIDATE_INPUTS)
        self.assertEqual(selected, [grant] + dust[:CONSOLIDATE_INPUTS])
        # Never past max(output, 2 * need): a 2.2e10 total stops the padding.
        selected, _ = cover([grant, out(9, 3 * 10**9)], self.NEED, pad=CONSOLIDATE_INPUTS)
        self.assertEqual(selected, [grant])
        # Pinned shapes and unpaddable outputs are never padded.
        self.assertEqual(cover([grant] + dust, self.NEED, 1, pad=2)[0], [grant])
        self.assertEqual(cover([grant] + dust, self.NEED, pad=2, paddable=lambda o: False)[0], [grant])

    def test_free_shape_combinations_skip_unpaddable_outputs(self):
        # #2023 review: a V2 award never joins a free-shape multi-input set.
        outputs = [out(1, 9 * 10**9), out(2, 2 * 10**9), out(3, 3 * 10**9)]
        award = lambda o: o["id"] != "0002"
        self.assertEqual([o["id"] for o in cover(outputs, self.NEED)[0]], ["0001", "0002"])
        self.assertEqual([o["id"] for o in cover(outputs, self.NEED, paddable=award)[0]],
                         ["0001", "0003"])
        # A pinned count may still use it.
        self.assertEqual([o["id"] for o in cover(outputs, self.NEED, 2, paddable=award)[0]],
                         ["0001", "0002"])

    def test_ranked_takes_the_first_covering_set_in_rank_order(self):
        outputs = [out(1, 5 * 10**10), out(2, 4 * 10**10), out(3, 6 * 10**10)]
        first = {"0002": 0}
        rank = lambda o: (first.get(o["id"], 1), -o["utxo"]["amount"])
        self.assertEqual(ranked(outputs, self.NEED, rank)[0], [outputs[1]])
        # None never spends an output; the rest go largest first.
        rank = lambda o: None if o["id"] == "0002" else (0, -o["utxo"]["amount"])
        self.assertEqual(ranked(outputs, self.NEED, rank)[0], [outputs[2]])
        self.assertEqual(ranked(outputs, 10**12, rank)[0], [])

    def test_decoy_check_can_refuse_padding(self):
        dust = [out(10, 10**9)]
        grant = out(1, 2 * 10**10)
        selected, _ = cover([grant] + dust, self.NEED, pool_ok=lambda s: len(s) == 1,
                            pad=CONSOLIDATE_INPUTS)
        self.assertEqual(selected, [grant])


if __name__ == "__main__":
    unittest.main()
