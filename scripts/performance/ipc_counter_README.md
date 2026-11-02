# Private WebView IPC counter

This Linux-only, opt-in `LD_PRELOAD` probe counts Wry/WebKitGTK `ipc` script
messages. It records no payload, command, URL, path, or application data. It
does not observe MCP, plugin, or other application IPC channels.

Wry 0.57 connects to the generic `script-message-received` signal, then
registers the `ipc` handler. The shim intercepts that real registration call
and, only when it succeeds, attaches a second observer to the exact
`script-message-received::ipc` detail. It leaves Wry's callback and user data
untouched. A per-manager GObject marker prevents duplicate observers.

The counter file is created with mode 0600 and `O_EXCL`; choose a new path for
each instrumented process. The shim activates only when all three environment
values are present and valid:

- `BURRATA_IPC_COUNTER_ENABLE=1`
- `BURRATA_IPC_COUNTER_PATH=/absolute/private/path/ipc-counter.json`
- `BURRATA_IPC_COUNTER_PID=<target application PID>`

The PID gate prevents inherited `LD_PRELOAD` in helper processes from creating
or updating the application counter. To set it before the target's loader
runs, use a shell that execs the app in place:

```sh
LD_PRELOAD=/absolute/path/libipc_counter.so \
BURRATA_IPC_COUNTER_ENABLE=1 \
BURRATA_IPC_COUNTER_PATH=/absolute/private/path/ipc-counter.json \
bash -c 'export BURRATA_IPC_COUNTER_PID=$$; exec "$@"' _ /absolute/path/to/burrata
```

The file contains only numeric `registered` and `count` fields. `registered`
is the number of WebKit user-content managers where the real `ipc`
registration succeeded and the detailed observer attached. `count` is the
cumulative number of matching signals for that process. Both values are
updated in place with `pwrite`; they can be read while the application runs.
For a 20-second idle window, snapshot the count after readiness and again at
the end, then subtract. A missing file means the opt-in/PID gate did not
activate; `registered: 0` means no matching observer attached, so neither is
evidence of zero IPC traffic. Use separate files/processes for baseline and
final. Do not reset the file while a process is running.

Build an uncommitted shared object and run the fixtures with:

```sh
scripts/performance/ipc_counter_build.sh /tmp/private-run/libipc_counter.so
scripts/performance/ipc_counter_test.sh
```

The script builds in a temporary directory, checks disabled and wrong-PID
gates, non-IPC/failed registration, duplicate registration, exact-detail counting, callback
forwarding and cleanup, and uses a real WebKitGTK manager when version 4.1 or
4.0 is installed. It does not launch Burrata or modify a user profile.
