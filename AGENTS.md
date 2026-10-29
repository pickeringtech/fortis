# Working in this repository (agents)

Instructions for any agent developing Fortis here: Claude Code reads them through `CLAUDE.md`, Codex and others
read this file directly. [CONTRIBUTING.md](CONTRIBUTING.md) says how to build Fortis and run the gate; the
`fortis-code` skill (`skills/fortis-code/SKILL.md`) says how to read, check and change Fortis source.

## Waiting on background work

Every wait has a deadline. A loop, watcher or poll that waits on something else MUST end on its own, in one of
three ways: the work succeeded, it failed, or the deadline passed. It reports which of the three happened, and on
a timeout it reports what it saw (whether the process is alive, how large the output is, the last lines of the
log), so the agent can decide whether to keep waiting. A wait with no deadline that hangs looks the same as work
still in progress, and nobody can tell them apart.

- **Prefer running the job itself in the background, under `timeout`.** Its own exit is then the notification,
  and no separate watcher is needed:

  ```bash
  timeout 90m python3 -I conformance/tools/gate.py --json build/gate.json > build/gate.log 2>&1
  echo "exit=$?"    # 124 means the deadline passed
  ```

- **When you have to watch work you did not start, wait on its PID, never on a pattern.**
  `pgrep -f "gate.py"` run from a shell whose own command line contains `gate.py` matches that shell, so the loop
  waits on itself forever. Grepping a log for "exit" or "done" fails in the opposite direction, firing early,
  because the tool's own output contains those words.

  ```bash
  pid=12345; deadline=$((SECONDS + 2700))
  while kill -0 "$pid" 2>/dev/null && [ "$SECONDS" -lt "$deadline" ]; do sleep 10; done
  if kill -0 "$pid" 2>/dev/null; then echo "TIMEOUT: $pid still running"; else echo "finished"; fi
  wc -c build/gate.log; tail -n 5 build/gate.log
  ```

- **Check in, even before the deadline.** When a wait passes the time the work usually takes, look at the output
  file and the process before waiting longer. Never report work as running without checking.
- Stop your own watchers once they have done their job.
- Never stop processes by name or pattern (`pkill -f`, `killall`): stop the PID you started.
