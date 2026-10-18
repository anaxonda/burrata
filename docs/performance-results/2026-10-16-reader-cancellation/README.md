# Native reader setup cancellation

Main source carries the original absolute startup deadline into native output loading and races initial HTTP setup against that deadline and the decoder cancellation flag. The active download cancellation bridge is installed before synchronous reader consumption. It does not set a total response-body timeout. Normal post-audio seeks clear the old startup deadline.

The immutable development-test executable in `native-manifest.json` compiled successfully and passed all 21 active native-output tests, with three opt-in tests ignored. These include pending-response-header cancellation, setup deadline expiry, stalled initial-prefetch cancellation, buffered seeks, range seeks and chapter bounds. The original header-deadline fixture passed here; a deterministic delayed-future/drop fixture is staged to remove its client-start scheduling race. A separate ready-seek watchdog improvement is merged after this binary and still needs compilation.

This is deterministic Linux native-output verification, not a full-suite checkpoint or live first-mixer latency/resource qualification. No new GUI was launched and the owned provider login was preserved.

After the newer combined 742-test checkpoint passed, this intermediate executable was removed following a host process/inode and hash check. Its source manifest and verification logs remain; the newer immutable executable retains the behavior plus the deterministic fixture and seek watchdog.
