# Back/Forward restoration race

A second history-navigation request could run while the first request was awaiting provider/local data. It captured partially restored controls, mutated the route stacks again and invalidated the first restoration. The controls stayed enabled throughout the wait.

The source now makes the restoration flag reactive, disables both history buttons during the operation, and guards overlapping calls before capturing a route or changing either stack. Normal navigation to a different view still cancels stale work through the existing epoch mechanism.

Two parameterized regressions defer the search-source response, attempt another Back or Forward while pending, then verify the query/provider restore and the subsequent Forward destination remains Settings. Both passed in the private stage. Main-worktree complete frontend verification passes: 873 tests across 74 files; Svelte zero errors and warnings. This fixes the identified race; it does not claim complete live navigation/scroll qualification for every provider view.

Evidence: `/tmp/navigation-race.patch`, `.cache/qualification-track-failure-history/frontend-navigation-all-tests.log`, and `navigation-source-change.json`. The running normal binary/frontend remains the earlier protected checkpoint until the next verified handoff.

The new normal build `ecc2c40a10e8` is running after verified restoration of the full30-entry queue/occurrences and paused3.287602s state. Read-only layout inspection at1914×515 reports no playback-bar findings (footer98.5px); root also reviewed the app-owned screenshot and found compact library controls and no footer overlap. This does not replace live Back/Forward/scroll acceptance. Evidence: `.cache/qualification-kaufman-ledger-current/normal-launch/{restoration.json,playback-layout-private.json,ui-private.png}`.
