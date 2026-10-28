# Configured Opus decoder capability

The native decoder worker rehashes the configured FFmpeg executable immediately before launching it. If an updater replaces the executable after playback planning, the worker fails closed before producing PCM and asks the user to replay the track so the new executable can be checked.

This pathname check detects same-path replacements between planning and worker launch. Portable process APIs cannot atomically bind the bytes hashed to the executable image opened by the following process-creation call, so a narrow replacement race remains between the final hash and OS process creation. Eliminating that interval requires platform-specific executable pinning with equivalent behavior on every supported target.
