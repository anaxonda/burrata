#!/usr/bin/env python3
"""Private experiment subprocess accounting. Never copies child stderr to reports."""
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import sys


def main():
    output, *command = sys.argv[1:]
    try:
        child = subprocess.Popen(command)
    except OSError:
        own = resource.getrusage(resource.RUSAGE_SELF)
        scale = 1 if sys.platform == 'darwin' else 1024
        Path(output).write_text(json.dumps({'childUnavailable': True, 'childCpuMs': 0, 'childMaxRssBytes': 0,
                                           'wrapperCpuMs': (own.ru_utime + own.ru_stime) * 1000,
                                           'wrapperMaxRssBytes': own.ru_maxrss * scale, 'accounting': 'wait4-reaped-command-tree'}))
        return 127
    def stop(_signum, _frame):
        child.terminate()
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    _, status, usage = os.wait4(child.pid, 0)
    code = os.waitstatus_to_exitcode(status)
    child.returncode = code
    # wait4 includes descendants reaped by this command. maxrss is a maximum,
    # not the sum of overlapping resident sets. Linux reports KiB; macOS bytes.
    own = resource.getrusage(resource.RUSAGE_SELF)
    scale = 1 if sys.platform == 'darwin' else 1024
    Path(output).write_text(json.dumps({
        'childCpuMs': (usage.ru_utime + usage.ru_stime) * 1000,
        'childMaxRssBytes': usage.ru_maxrss * scale,
        'wrapperCpuMs': (own.ru_utime + own.ru_stime) * 1000,
        'wrapperMaxRssBytes': own.ru_maxrss * scale,
        'accounting': 'wait4-reaped-command-tree',
    }))
    return code if code >= 0 else 128 - code


if __name__ == '__main__':
    sys.exit(main())
