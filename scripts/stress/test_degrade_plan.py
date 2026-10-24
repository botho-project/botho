"""Offline checks for the scoped x-series validator-degrade schedule (#1585)."""

import copy
import hashlib
import json
from pathlib import Path
import unittest

from plan import expand
from runtime import HOSTS

HERE = Path(__file__).parent
PHASE_START = 48 * 3600

# Expansion of the existing tracked plans captured at ff893a82, before the
# degrade schema existed: plan_sha256, sha256 of the `plan.py --output`
# summary JSON, sha256 of the intents.jsonl bytes, and the event count.
GOLDEN = {
    "testnet-discovery-v2.json": (
        "b459df6e38eb363d5e55ef03feb71dd0470c4d53c5d469787bc8be30506d3c08",
        "414ba3e536180bd57ededf90c61ea58439f1c84d8f95ac681f257ce5425ac7bd",
        "a5302984ac4c4fd149a842ad49ff86278881180c98d8479dcb7b90f3bcedd997",
        22680,
    ),
    "testnet-72h-plan.json": (
        "75525bacd3ec80bb76f3b7cba8f67d604dceb9b16c456b6fa13af4f272e9c3fc",
        "c64f95381de61a34efa521649c8f255e9865ed6b1029b4817491f5a49a5a4101",
        "a5dd2942931ebadf036c554c24b8016258a9e524b94bad2860d7c27233160920",
        692,
    ),
}

# (id, action, targets, fault start, restore, window end, offers, expect),
# seconds relative to the degrade phase start (hour 48).
SCHEDULE = [
    ("D1", "stop_restart", ["n1"], 0, 1800, 3600, "continue", "progress"),
    ("D2", "kill_restart", ["n2"], 3600, 4200, 6000, "continue", "progress"),
    ("D3", "isolate", ["n4"], 6000, 7800, 9600, "continue", "progress"),
    ("D4", "netem", ["n5"], 9600, 11400, 13200, "continue", "progress"),
    ("D5", "stop_restart_pair", ["n1", "n2"], 13500, 14100, 15900, "none", "safe_halt"),
]


def load(name):
    return json.loads((HERE / name).read_text())


class DegradePlanTests(unittest.TestCase):
    def plan(self):
        return load("x-series-plan.json")

    def rejects(self, change, plan=None):
        plan = copy.deepcopy(plan or self.plan())
        change(plan)
        with self.assertRaises(ValueError):
            expand(plan)

    def step(self, plan, step_id):
        return next(s for s in plan["degrade"]["steps"] if s["id"] == step_id)

    # -- the tracked x-series plan ------------------------------------------

    def test_x_series_plan_expands_with_the_d1_d5_schedule(self):
        summary, events = expand(self.plan())
        self.assertEqual(summary["nominal_campaign_payments"], 22480)
        self.assertEqual(len(events), 22480)
        self.assertEqual(
            [(p["phase"], p["start_hour"], p["end_hour"]) for p in summary["phases"][-3:]],
            [("recovery", 42, 48), ("degrade", 48, 54), ("repeat", 54, 72)],
        )
        steps = summary["degrade_steps"]
        self.assertEqual(
            [
                (
                    s["id"],
                    s["action"],
                    s["targets"],
                    s["fault_start_offset"],
                    s["restore_offset"],
                    s["window_end_offset"],
                    s["offers"],
                    s["expect"],
                )
                for s in steps
            ],
            [
                (i, a, t, PHASE_START + f, PHASE_START + r, PHASE_START + w, o, e)
                for i, a, t, f, r, w, o, e in SCHEDULE
            ],
        )
        for s in steps:
            self.assertEqual(
                s["target_hosts"], [HOSTS[int(t[1:]) - 1] for t in s["targets"]]
            )
            self.assertNotIn("n3", s["targets"])
        self.assertEqual(steps[3]["loss_percent"], 20)
        self.assertEqual(steps[3]["delay_ms"], 250)
        self.assertNotIn("loss_percent", steps[0])
        # D5's quiet lead and window carry no offers.
        d5 = steps[4]
        self.assertFalse(
            [
                e
                for e in events
                if d5["fault_start_offset"] - 300
                <= e["offset_seconds"]
                < d5["window_end_offset"]
            ]
        )

    def test_x_series_changes_only_the_opt_in_phases_and_degrade_block(self):
        base, plan = load("testnet-discovery-v2.json"), self.plan()
        for key in set(base) | set(plan):
            if key not in ("faults", "phases", "degrade"):
                self.assertEqual(plan[key], base[key], key)
        faults = dict(plan["faults"])
        self.assertIs(faults.pop("validator_degrade_phase"), True)
        self.assertEqual(faults, base["faults"])
        self.assertEqual(plan["phases"][:-2], base["phases"][:-1])

    def test_rehearsal_only_keeps_validation_but_schedules_no_fault(self):
        plan = self.plan()
        plan.update(execution_mode="rehearsal_only", setup_deadline_hours=1)
        summary, events = expand(plan)
        self.assertEqual((events, summary["degrade_steps"]), ([], []))
        self.rejects(
            lambda p: self.step(p, "D5").update(start_seconds=13200), plan
        )

    # -- existing plans are untouched --------------------------------------

    def test_existing_plans_expand_byte_identically(self):
        for name, (plan_sha, summary_sha, events_sha, count) in GOLDEN.items():
            summary, events = expand(load(name))
            self.assertNotIn("degrade_steps", summary)
            self.assertEqual(summary["plan_sha256"], plan_sha, name)
            self.assertEqual(
                hashlib.sha256(json.dumps(summary, indent=2).encode()).hexdigest(),
                summary_sha,
                name,
            )
            self.assertEqual(
                hashlib.sha256(
                    "".join(json.dumps(e) + "\n" for e in events).encode()
                ).hexdigest(),
                events_sha,
                name,
            )
            self.assertEqual(len(events), count)

    def test_explicit_false_opt_in_without_block_is_accepted(self):
        plan = load("testnet-discovery-v2.json")
        plan["faults"]["validator_degrade_phase"] = False
        summary, _ = expand(plan)
        self.assertNotIn("degrade_steps", summary)

    # -- scope of the opt-in -------------------------------------------------

    def test_schema_v1_rejects_the_opt_in(self):
        v1 = load("testnet-72h-plan.json")
        expand(copy.deepcopy(v1))
        self.rejects(lambda p: p["faults"].update(validator_degrade_phase=True), v1)
        self.rejects(lambda p: p.update(degrade=self.plan()["degrade"]), v1)

    def test_opt_in_with_any_public_fault_is_rejected(self):
        for key in (
            "public_automatic_node_stops",
            "public_miner_changes",
            "public_partitions",
            "public_disk_faults",
        ):
            self.rejects(lambda p, k=key: p["faults"].update({k: True}))

    def test_opt_in_requires_isolated_only_and_private_endpoints(self):
        self.rejects(lambda p: p.update(isolated_only=False))
        self.rejects(
            lambda p: p["endpoints"].__setitem__(0, "https://" + HOSTS[0] + "/rpc")
        )

    def test_opt_in_and_block_require_each_other(self):
        self.rejects(lambda p: p.pop("degrade"))
        self.rejects(lambda p: p["faults"].pop("validator_degrade_phase"))
        self.rejects(lambda p: p["faults"].update(validator_degrade_phase=False))
        self.rejects(lambda p: p["faults"].update(validator_degrade_phase="true"))

    # -- one negative per validation rule ------------------------------------

    def test_overlap_including_recovery_is_rejected(self):
        # D2 starts while D1 is still in its recovery window.
        self.rejects(lambda p: self.step(p, "D2").update(start_seconds=3599))
        # Steps out of order.
        self.rejects(lambda p: p["degrade"]["steps"].reverse())

    def test_producer_target_is_rejected(self):
        self.rejects(lambda p: self.step(p, "D1").update(targets=["n3"]))
        self.rejects(lambda p: self.step(p, "D5").update(targets=["n1", "n3"]))
        self.rejects(lambda p: p["degrade"].update(producer_role="n1"))

    def test_wrong_target_count_or_role_is_rejected(self):
        self.rejects(lambda p: self.step(p, "D1").update(targets=["n1", "n2"]))
        self.rejects(lambda p: self.step(p, "D1").update(targets=[]))
        self.rejects(lambda p: self.step(p, "D5").update(targets=["n1"]))
        self.rejects(lambda p: self.step(p, "D5").update(targets=["n1", "n1"]))
        self.rejects(lambda p: self.step(p, "D1").update(targets=["seed.botho.io"]))

    def test_pair_step_must_be_safe_halt_with_no_offers(self):
        self.rejects(lambda p: self.step(p, "D5").update(expect="progress"))
        self.rejects(lambda p: self.step(p, "D5").update(offers="continue"))

    def test_offer_inside_quiet_lead_or_window_of_none_step_is_rejected(self):
        # Inside the 300 s lead: the first degrade segment's last offer is
        # +13185, so a fault at +13485 puts it 300 s ahead (inclusive).
        self.rejects(lambda p: self.step(p, "D5").update(start_seconds=13485))
        # Inside the window: offers continue through the step.
        def no_gap(p):
            p["phases"][-2]["segments"][0]["count"] = 1240
            del p["phases"][-2]["segments"][1]

        self.rejects(no_gap)
        # A step with offers continuing is fine with the same timing.
        plan = self.plan()
        self.step(plan, "D5").update(start_seconds=13485)
        self.step(plan, "D5")["action"] = "stop_restart"
        self.step(plan, "D5").update(targets=["n1"], offers="continue", expect="progress")
        expand(plan)

    def test_step_outside_its_phase_is_rejected(self):
        # Recovery runs past the 54 h boundary.
        self.rejects(lambda p: self.step(p, "D5").update(start_seconds=19801))
        self.rejects(lambda p: self.step(p, "D1").update(start_seconds=-1))

    def test_degrade_phase_naming_no_phase_is_rejected(self):
        self.rejects(lambda p: p["degrade"].update(phase="nonexistent"))

    def test_out_of_bounds_durations_are_rejected(self):
        for field, value in [
            ("fault_seconds", 59),
            ("fault_seconds", 3601),
            ("recovery_seconds", 299),
            ("recovery_seconds", 3601),
            ("fault_seconds", 600.0),
            ("recovery_seconds", True),
        ]:
            self.rejects(lambda p, f=field, v=value: self.step(p, "D1").update({f: v}))

    def test_netem_parameters_are_required_and_bounded(self):
        self.rejects(lambda p: self.step(p, "D4").pop("loss_percent"))
        self.rejects(lambda p: self.step(p, "D4").pop("delay_ms"))
        for field, value in [
            ("loss_percent", 51),
            ("loss_percent", -1),
            ("loss_percent", 20.0),
            ("delay_ms", 2001),
            ("delay_ms", "250"),
        ]:
            self.rejects(lambda p, f=field, v=value: self.step(p, "D4").update({f: v}))
        # netem parameters on a non-netem step are an error, not ignored.
        self.rejects(lambda p: self.step(p, "D1").update(loss_percent=10, delay_ms=0))

    def test_unknown_or_malformed_fields_are_rejected(self):
        self.rejects(lambda p: self.step(p, "D1").update(action="reboot"))
        self.rejects(lambda p: self.step(p, "D1").update(signal="SIGINT"))
        self.rejects(lambda p: self.step(p, "D1").update(offers="some"))
        self.rejects(lambda p: self.step(p, "D1").update(expect="halt"))
        self.rejects(lambda p: self.step(p, "D2").update(id="D1"))
        self.rejects(lambda p: p["degrade"].update(steps=[]))
        self.rejects(lambda p: p["degrade"].update(extra=1))


if __name__ == "__main__":
    unittest.main()
