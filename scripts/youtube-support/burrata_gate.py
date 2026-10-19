"""Burrata's managed readiness gate; upstream token semantics stay unchanged."""
import functools
from urllib.parse import urlparse
from yt_dlp.extractor.youtube.pot.provider import register_provider, register_preference
from yt_dlp_plugins.extractor.getpot_bgutil_http import BgUtilHTTPPTP


@register_provider
class BurrataGatePTP(BgUtilHTTPPTP):
    PROVIDER_NAME = 'burrata:gate'
    # Only loopback readiness waiting changes. Rust owns the original absolute
    # playback deadline and closes the lease on cancellation; no fresh budget.
    _GET_SERVER_VSN_TIMEOUT = 20.0

    @functools.cached_property
    def _base_url(self):
        value = self.ie._configuration_arg(
            ie_key='youtubepot-burratagate', key='base_url', default=[''])[0]
        parsed = urlparse(value)
        if (parsed.scheme != 'http' or parsed.hostname != '127.0.0.1'
                or not parsed.port or not parsed.path.strip('/')
                or parsed.username or parsed.password or parsed.query or parsed.fragment):
            self._warn_and_raise('A managed loopback readiness lease is required')
        return value.rstrip('/')

    def is_available(self):
        configured = self.ie._configuration_arg(
            ie_key='youtubepot-burratagate', key='base_url', default=[''])[0]
        return bool(configured) and super().is_available()

    def _check_version(self, got_version, *, default='unknown', name):
        if self.PROVIDER_VERSION != '2.0.2' or got_version != '2.0.2':
            self._warn_and_raise('Managed readiness gate requires the pinned 2.0.2 provider and server')
        return super()._check_version(got_version, default=default, name=name)


@register_preference(BurrataGatePTP)
def burrata_gate_preference(provider, request):
    return 1000


__all__ = [BurrataGatePTP.__name__, burrata_gate_preference.__name__]
