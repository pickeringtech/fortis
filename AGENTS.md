# Working in this repository (agents)

Instructions for any agent developing Fortis here: Claude Code reads them through `CLAUDE.md`, Codex and others
read this file directly.

## Waiting on background work

Every wait has a deadline. A loop, watcher or poll that waits on something else MUST end on its own, in one of
three ways: the work succeeded, it failed, or the deadline passed. It reports which of the three happened, and on
a timeout it reports what it saw (whether the process is alive, how large the output is, the last lines of the
log), so the agent can decide whether to keep waiting. A wait with no deadline that hangs looks the same as work
still in progress, and nobody can tell them apart.

- **Prefer running the job itself in the background, under `timeout`.** Its own exit is then the notification,
  and no separate watcher is needed:

  ```bash
  timeout 45m codex exec -s read-only - < docs/reviews/x-prompt.md > docs/reviews/x-codex.md 2> "$TMP/x.err"
  echo "exit=$?"    # 124 means the deadline passed
  ```

- **When you have to watch work you did not start, wait on its PID, never on a pattern.**
  `pgrep -f "codex exec"` run from a shell whose own command line contains `codex exec` matches that shell, so the
  loop waits on itself forever. Grepping a transcript for "exit" or "done" fails in the opposite direction, firing
  early, because the tool's own output contains those words.

  ```bash
  pid=12345; deadline=$((SECONDS + 2700))
  while kill -0 "$pid" 2>/dev/null && [ "$SECONDS" -lt "$deadline" ]; do sleep 10; done
  if kill -0 "$pid" 2>/dev/null; then echo "TIMEOUT: $pid still running"; else echo "finished"; fi
  wc -c docs/reviews/x-codex.md; tail -n 5 "$TMP/x.err"
  ```

- **Check in, even before the deadline.** When a wait passes the time the work usually takes, look at the output
  file and the process before waiting longer. Never report work as running without checking.
- Stop your own watchers once they have done their job.

## External reviews

Design and spike reviews run Codex (`codex exec -s read-only - < prompt > docs/reviews/<name>-codex.md`) from the
repository root, under the rule above. Read the whole output, and address or record every finding: in a spike's
design before preregistration, or in its decision record's post-review table after the run.
