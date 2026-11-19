import json
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from unittest.mock import patch

from verify_x2_memory import (
    bind,
    check_archive_index,
    check_fleet_shape,
    file_hash,
    verify,
)


class QualificationBindingTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.binary = self.root / "node"
        self.binary.write_bytes(b"candidate")
        self.run = "q20260101-x2-rehearsal"

    def test_concurrent_retries_share_one_complete_receipt(self):
        receipt = {"artifact_sha256": file_hash(self.binary)}
        with patch("verify_x2_memory.verify", return_value=receipt):
            with ThreadPoolExecutor(4) as pool:
                results = list(
                    pool.map(
                        lambda _: bind(self.root, self.run, self.root, self.binary),
                        range(8),
                    )
                )
            self.assertEqual(results, [receipt] * 8)
            self.assertEqual(bind(self.root, self.run), receipt)
        with patch(
            "verify_x2_memory.verify", return_value={"artifact_sha256": "changed"}
        ):
            with self.assertRaisesRegex(ValueError, "frozen receipt"):
                bind(self.root, self.run, self.root, self.binary)
        saved = json.loads(
            (self.root / "payment-memory-qualification.json").read_text()
        )
        self.assertEqual(saved["receipt"], receipt)

    def test_detached_export_without_final_receipt_is_held(self):
        for folder in ("outcome", ".outcome-partial"):
            with self.subTest(folder=folder):
                archive = self.root / folder
                archive.mkdir()
                (archive / "driver-config.json").write_text("{}")
                with self.assertRaisesRegex(ValueError, "published outcome"):
                    verify(archive, self.binary)

    def test_absent_archive_cannot_create_receipt(self):
        with self.assertRaisesRegex(ValueError, "requires"):
            bind(self.root, self.run)
        self.assertFalse((self.root / "payment-memory-qualification.json").exists())

    def test_mismatched_node_is_refused_before_replay(self):
        from test_sustained_memory import manifest

        archive = manifest("minting")
        (self.root / "manifest.json").write_text(json.dumps(archive))
        with self.assertRaisesRegex(ValueError, "candidate node SHA-256"):
            verify(self.root, self.binary)

    def test_checksum_index_rejects_missing_driver_result(self):
        (self.root / "SHA256SUMS").write_text("0" * 64 + "  ./result.json\n")
        with self.assertRaises(FileNotFoundError):
            check_archive_index(self.root, {})

    def test_inventory_must_be_from_the_same_cohort_and_instance_types(self):
        roles = [
            {"role": f"n{i}", "type": "producer" if i == 3 else "validator"}
            for i in range(1, 6)
        ]
        inventory = [
            dict(r, id=str(i), hostname=f"{r['role']}.{self.run}.internal")
            for i, r in enumerate(roles)
        ]
        self.assertEqual(
            check_fleet_shape(inventory, self.run, roles)["n3"], "producer"
        )
        for changes in (
            {"hostname": "n3.other.internal"},
            {"id": "1"},
            {"type": "validator"},
        ):
            with (
                self.subTest(changes=changes),
                self.assertRaisesRegex(ValueError, "inventory"),
            ):
                check_fleet_shape(
                    [*inventory[:2], dict(inventory[2], **changes), *inventory[3:]],
                    self.run,
                    roles,
                )

    def test_outer_receipt_binds_checksum_index(self):
        archive = self.root / "outcome"
        archive.mkdir()
        (archive / "SHA256SUMS").write_text("")
        (self.root / "final-receipt.json").write_text(
            json.dumps({"checksum_index_sha256": "wrong"})
        )
        with self.assertRaisesRegex(ValueError, "final receipt"):
            check_archive_index(archive, {})

    def test_recovered_outcome_supplies_inventory_for_fresh_operator(self):
        run = "q20261107-x2-memqual9"
        fleet = self.root / "fresh-operator" / self.run
        fleet.mkdir(parents=True)
        archive = self.root / "evidence" / run / "outcome"
        archive.mkdir(parents=True)
        inventory = [
            {
                "role": f"n{i}",
                "id": str(i),
                "type": "large",
                "hostname": f"n{i}.{run}.internal",
            }
            for i in range(1, 6)
        ]
        roles = [{"role": n["role"], "type": n["type"]} for n in inventory]
        sidecar = archive.parent / "inventory.json"
        sidecar.write_text(json.dumps(inventory))
        with patch("verify_x2_memory.verify", return_value={"qualification_run": run}):
            with self.assertRaises(FileNotFoundError):
                bind(fleet, self.run, archive, self.binary, roles=roles)
            (archive.parent / "final-receipt.json").write_text("{}")
            receipt = bind(fleet, self.run, archive, self.binary, roles=roles)
            self.assertEqual(
                receipt["validator_types"], {n["role"]: "large" for n in roles}
            )
            self.assertEqual(bind(fleet, self.run, roles=roles), receipt)
            inventory[2]["hostname"] = "n3.other.internal"
            sidecar.write_text(json.dumps(inventory))
            with self.assertRaisesRegex(ValueError, "inventory"):
                bind(fleet, self.run, roles=roles)


if __name__ == "__main__":
    unittest.main()
