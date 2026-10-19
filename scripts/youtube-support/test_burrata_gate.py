"""Offline contract tests for the maintained shim against the packaged plugin."""
import importlib.util
from pathlib import Path
import sys
import unittest
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'src-tauri/resources/youtube-support/bgutil/plugin'))
spec = importlib.util.spec_from_file_location('burrata_gate', Path(__file__).with_name('burrata_gate.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
from yt_dlp.extractor.youtube.pot.provider import PoTokenProviderRejectedRequest


class GateTests(unittest.TestCase):
    def provider(self, url='http://127.0.0.1:34567/exact-lease'):
        ie = SimpleNamespace(_configuration_arg=lambda **kwargs: [url])
        logger = SimpleNamespace(warning=lambda *args, **kwargs: None)
        return module.BurrataGatePTP(ie, logger, {})

    def test_loopback_nonce_and_inherited_token_transport(self):
        provider = self.provider()
        self.assertEqual(provider._base_url, 'http://127.0.0.1:34567/exact-lease')
        self.assertEqual(provider._GET_SERVER_VSN_TIMEOUT, 20.0)
        self.assertIs(module.BurrataGatePTP._real_request_pot, module.BgUtilHTTPPTP._real_request_pot)
        self.assertEqual(module.BgUtilHTTPPTP._GET_SERVER_VSN_TIMEOUT, 5.0)

    def test_no_gate_for_flat_or_ordinary_calls(self):
        self.assertFalse(self.provider('').is_available())

    def test_remote_credentials_or_missing_lease_rejected(self):
        for value in ['https://127.0.0.1:34567/lease', 'http://example.com:34567/lease',
                      'http://127.0.0.1:34567/', 'http://user@127.0.0.1:34567/lease',
                      'http://127.0.0.1:34567/lease?different=1']:
            with self.subTest(value=value), self.assertRaises(PoTokenProviderRejectedRequest):
                self.provider(value)._base_url

    def test_exact_pinned_server_compatibility(self):
        provider = self.provider()
        provider._check_version('2.0.2', name='fixture')
        for version in ['2.0.3', '3.0.0', '']:
            with self.subTest(version=version), self.assertRaises(PoTokenProviderRejectedRequest):
                provider._check_version(version, name='fixture')


if __name__ == '__main__':
    unittest.main()
