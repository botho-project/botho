#!/usr/bin/env python3
"""Exercise driver selection without Rust builds, nodes, credentials or funds."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
BRIDGE = "fork_tests::fork_eth_mint_and_burn_round_trip"
UNISWAP = "uniswap_fork_tests::uniswap_fork_pool_create_add_liquidity_and_swap"


class DriverSelection(unittest.TestCase):
    def run_driver(self, name, cargo_status=0):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "scripts").mkdir()
            (root / "contracts/ethereum/node_modules").mkdir(parents=True)
            shutil.copy(ROOT / "scripts" / name, root / "scripts" / name)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            for command in ("cargo", "npx"):
                stub = bin_dir / command
                stub.write_text(
                    "#!/usr/bin/env python3\n"
                    "import json, os, sys\n"
                    "with open(os.environ['COMMAND_LOG'], 'a') as log:\n"
                    "    log.write(json.dumps(sys.argv) + '\\n')\n"
                    "sys.exit(int(os.environ.get('CARGO_STATUS', '0')) "
                    "if sys.argv[0].endswith('/cargo') else 0)\n"
                )
                stub.chmod(0o755)
            env = dict(os.environ, PATH=f"{bin_dir}:{os.environ['PATH']}",
                       COMMAND_LOG=str(root / "commands.jsonl"),
                       CARGO_STATUS=str(cargo_status),
                       BRIDGE_FORK_RPC_URL="http://127.0.0.1:18545",
                       SEPOLIA_RPC_URL="http://unused.invalid")
            result = subprocess.run(["bash", str(root / "scripts" / name)],
                                    env=env, capture_output=True, text=True)
            calls = [json.loads(line) for line in
                     (root / "commands.jsonl").read_text().splitlines()]
            return result, [args[1:] for args in calls if args[0].endswith('/cargo')]

    def test_local_only_runs_provisioned_bridge_test(self):
        result, calls = self.run_driver("bridge-e2e-local.sh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [["test", "-p", "bth-bridge-service", "--lib", "--",
                                 "--ignored", "--exact", BRIDGE, "--nocapture"]])

    def test_sepolia_retains_bridge_and_uniswap_serially(self):
        result, calls = self.run_driver("bridge-e2e-fork.sh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [["test", "-p", "bth-bridge-service", "--lib", "--",
                                 "--ignored", "--exact", BRIDGE, UNISWAP,
                                 "--test-threads=1", "--nocapture"]])

    def test_transaction_failure_is_not_masked(self):
        for driver in ("bridge-e2e-local.sh", "bridge-e2e-fork.sh"):
            with self.subTest(driver=driver):
                result, _ = self.run_driver(driver, cargo_status=101)
                self.assertEqual(result.returncode, 101)
                self.assertNotIn("e2e passed", result.stdout)


if __name__ == "__main__":
    unittest.main()
