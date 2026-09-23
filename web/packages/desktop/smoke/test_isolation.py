"""Static isolation guards; these deliberately do not claim native execution."""
import json
from pathlib import Path
import re
import tomllib
import unittest

DESKTOP = Path(__file__).resolve().parents[1]


class IsolationTests(unittest.TestCase):
    def test_production_entry_has_no_automation(self):
        for name in ('lib.rs', 'main.rs'):
            text = (DESKTOP / 'src-tauri/src' / name).read_text()
            self.assertNotIn('runtime_smoke', text)
            self.assertNotIn('wdio', text)
            self.assertNotIn('fixture_', text)
        cargo = tomllib.loads((DESKTOP / 'src-tauri/Cargo.toml').read_text())
        self.assertEqual(cargo['package']['default-run'], 'botho-desktop')
        self.assertEqual(cargo['bin'][0]['required-features'], ['runtime-smoke'])
        self.assertNotIn('runtime-smoke', cargo['features'].get('default', []))
        self.assertEqual(cargo['dependencies']['tauri-plugin-wdio-webdriver']['version'], '=1.4.0')
        self.assertTrue(cargo['dependencies']['tauri-plugin-wdio-webdriver']['optional'])

    def test_exact_command_allowlist(self):
        text = (DESKTOP / 'src-tauri/src/bin/runtime_smoke.rs').read_text()
        handler = re.search(r'generate_handler!\[(.*?)\]', text, re.S).group(1)
        names = [name.strip() for name in handler.split(',') if name.strip()]
        self.assertEqual(names, ['fixture_info', 'fixture_unlock', 'fixture_preflight',
                                 'wallet::get_session_status', 'wallet::lock_wallet'])
        self.assertIn('path: Some(path)', text)
        self.assertIn('.incognito(true)', text)
        self.assertNotIn('get_default_wallet_path', text)
        self.assertNotIn('app_lib::run()', text)

    def test_fixture_lifetime_is_owned_outside_app_state(self):
        text = (DESKTOP / 'src-tauri/src/bin/runtime_smoke.rs').read_text()
        fields = re.search(r'struct Fixture \{(.*?)\}', text, re.S).group(1)
        self.assertNotIn('TempDir', fields)
        self.assertNotIn('_directory', fields)
        run = text.index('let code = app.run_return(')
        close = text.index('directory.close()?;')
        exit_check = text.index('anyhow::ensure!(code == 0')
        self.assertLess(run, close)
        self.assertLess(close, exit_check)

    def test_separate_packaged_entry(self):
        config = json.loads((DESKTOP / 'src-tauri/tauri.smoke.conf.json').read_text())
        self.assertEqual(config['identifier'], 'com.botho.desktop.runtime-smoke')
        self.assertEqual(config['app']['windows'], [])
        self.assertEqual(config['build'], {'devUrl': None, 'frontendDist': '../smoke-dist'})
        self.assertFalse(config['bundle']['active'])
        self.assertNotIn('https:', config['app']['security']['csp'])
        text = (DESKTOP / 'smoke/main.tsx').read_text()
        for forbidden in ('ConnectionProvider', 'WalletProvider', "from '../src/App'", 'mockIPC'):
            self.assertNotIn(forbidden, text)
        self.assertIn("from '@tauri-apps/api/core'", text)
        self.assertIn("from '@botho/features/wallet'", text)
        self.assertIn("from '../src/components/network/network-graph'", text)


if __name__ == '__main__':
    unittest.main()
