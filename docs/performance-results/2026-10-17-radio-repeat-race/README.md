# Queue radio repeat-mode race

Added an App integration regression holding a native queue-radio recommendation across natural queue completion, changing repeat off → all → off, then releasing the recommendation. The stale result must not dispatch a playback play command. This protects against treating the final repeat-off value as permission to revive the earlier generation.

The new test passes in the current worktree. The related App radio selection passes28 tests; that name-filtered invocation skips the controller file, so the complete controller file was run separately and passes9 tests. Logs: `.cache/qualification-radio-repeat-race/{radio-tests.log,controller-tests.log}`. Only the integration test changed; production source and the running app are unchanged.

This is deterministic regression evidence, not live queue-end radio acceptance. Actual natural-end behavior, provider recommendations and broad repeat/continuation UI acceptance remain open.
