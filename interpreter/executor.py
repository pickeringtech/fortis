"""The cooperative executor (DEL-23: Language Reference 12.3–12.6).

Every task of one scope tree runs on one logical thread: a task runs until it completes or `await`s
something pending, and the scheduler then picks the next runnable task in launch order. Tasks are
Python threads that pass a single baton, so the interleaving is deterministic and the evaluator's
recursion stays ordinary. Cancellation is a flag observed only at an `await`: a cancelled task resumes
at its await, raises `CancelSignal`, unwinds (defers, abandon, drops) and completes as cancelled."""
from __future__ import annotations

import threading
from typing import Any, List, Optional


class CancelSignal(BaseException):
    """Cancellation observed at an await: unwinds the task; never a Fortis Failure (12.4)."""


class DefectSignal(BaseException):
    """A defect (10.8): terminates the task, is never caught, produces no completion value."""
    def __init__(self, kind: str):
        super().__init__(kind)
        self.kind = kind


SCOPE_BOUND = 256            # tasks a scope may hold (launched, not yet joined) and detached tasks a program may queue (PROG-NFR-13)
OBSERVATION_BOUND = 1000     # records the task reporter buffers before overflow becomes visible (PROG-NFR-13)


class FutureVal:
    """An async call that has not run: arguments bound, body pending (12.3)."""
    def __init__(self, fname: str, head, body, env, is_try: bool):
        self.fname, self.head, self.body, self.env, self.is_try = fname, head, body, env, is_try
        self.polled = False
        self.task: Optional["TaskVal"] = None
        # a closure this future owns (launched work, or an async closure a `move f()` call consumed): destroyed when
        # the body has run, or with the future if it never runs (7.9, X-250; D-30)
        self.consumes = None
        self.released = False      # its owned arguments and consumed closure were destroyed without running (D-30)

    def display(self) -> str:
        return f"Future({self.fname})"


class TaskVal:
    """A launched future (12.5): linear, awaited inside its scope or detached."""
    def __init__(self, future: Optional[FutureVal], name: str):
        self.future, self.name = future, name
        self.state = "ready"                  # ready | running | suspended | done
        self.result: Any = None
        self.error: Optional[BaseException] = None
        self.completion = "Completed"         # Completed | Cancelled | Defect
        self.cancelled = False
        self.waiting_on: Optional["TaskVal"] = None
        self.sleep_until: Optional[int] = None
        self.wake = threading.Event()
        self.thread: Optional[threading.Thread] = None
        self.detached = False
        self.scope: Optional["ScopeVal"] = None
        self.executor = "cooperative"          # cooperative | parallel | blocking (12.5)
        self.ignores_cancel = False            # waiting for a cancelled join's futures (12.5)
        # per-task evaluator state
        self.frames: List[Any] = []
        self.stack: List[str] = []
        self.cleanup_depth = 0
        self.temps: List[dict] = []       # statement temporary scopes (7.9): a stack of {id(value): value}, creation order

    @property
    def done(self) -> bool:
        return self.state == "done"

    def display(self) -> str:
        return f"Task({self.name})"


class SleepVal:
    """`clock.sleep(d)` awaited: the task suspends until virtual time reaches `until` (controlled scheduler)."""
    def __init__(self, ns: int):
        self.ns = ns

    def display(self) -> str:
        return f"Sleep({self.ns}ns)"


class JoinVal:
    """`join(a, b)` not yet awaited: both futures run to completion inside the parent when it is (12.5)."""
    def __init__(self, futures: List[FutureVal]):
        self.futures = futures

    def display(self) -> str:
        return "Join"


class ScopeVal:
    def __init__(self, name: str):
        self.name = name
        self.tasks: List[TaskVal] = []

    def display(self) -> str:
        return f"Scope({self.name})"


class Scheduler:
    def __init__(self, interp):
        self.it = interp
        self.main = TaskVal(None, "main")
        self.main.state = "running"
        self.current = self.main
        self.all: List[TaskVal] = []
        self.counter = 0
        self.detached: List[TaskVal] = []
        self.now = 0                           # virtual time in nanoseconds; advances only when nothing can run

    # ---- trace ----------------------------------------------------------------
    def trace(self, event: str):
        self.it.trace.append(event)

    # ---- creation ---------------------------------------------------------------
    def launch(self, fut: FutureVal, scope: Optional[ScopeVal], executor: str = "cooperative") -> TaskVal:
        if scope is not None and len(scope.tasks) >= SCOPE_BOUND:
            raise DefectSignal("out_of_memory")      # the scope queue has a declared bound; overflow is a visible defect
        self.counter += 1
        t = TaskVal(fut, f"{fut.fname}#{self.counter}")
        fut.task = t
        fut.polled = True
        t.scope = scope
        t.executor = executor
        self.all.append(t)
        if scope is not None:
            scope.tasks.append(t)
        self.trace(f"launch {t.name}" + ("" if executor == "cooperative" else f" on {executor}"))
        return t

    def checkpoint(self):
        """`checkpoint()` (12.4): a cancellation point for parallel and blocking tasks, modelled as a yield to
        the scheduler so the parent may act; cooperative tasks observe cancellation only at `await`."""
        me = self.current
        if me.executor == "cooperative":
            return
        self.trace(f"checkpoint {me.name}")
        if me.cancelled:
            self.trace(f"cancelled {me.name}")
            raise CancelSignal()
        me.state = "suspended"
        me.waiting_on, me.sleep_until = None, self.now
        nxt = next((t for t in self.all + [self.main] if t is not me and t.state == "suspended" and self._can_run(t)), None) or next((t for t in self.all if t is not me and t.state == "ready"), None)
        if nxt is None:
            me.state = "running"
            me.sleep_until = None
            return
        self._switch_to(nxt)
        me.state, me.waiting_on, me.sleep_until = "running", None, None
        if me.cancelled:
            self.trace(f"cancelled {me.name}")
            raise CancelSignal()

    def _body(self, t: TaskVal):
        t.wake.wait()
        t.wake.clear()
        try:
            if t.cancelled:
                self.trace(f"cancelled {t.name}")      # cancelled before it started: never polled
                t.completion = "Cancelled"
                if t.future is not None:
                    self.it.not_started(t.future)
            else:
                self.trace(f"start {t.name}")
                t.result = self.it.run_future(t.future)
        except CancelSignal:
            t.completion = "Cancelled"
        except RecursionError:
            t.error = DefectSignal("stack_exhaustion")
            t.completion = "Defect"
        except BaseException as e:  # noqa: BLE001  (defects and dynamic failures reach the awaiting task)
            t.error = e
            t.completion = "Defect"
        t.state = "done"
        self.trace(f"complete {t.name}")
        if t.detached:
            self.it.task_completion(t)
        self._pick_next(t)                       # hand the baton on; this thread ends

    def _start_thread(self, t: TaskVal):
        t.thread = threading.Thread(target=self._body, args=(t,), daemon=True)
        t.thread.start()

    # ---- switching ----------------------------------------------------------------
    def _can_run(self, t: TaskVal) -> bool:
        if t.state == "ready":
            return True
        if t.state != "suspended":
            return False
        if t.cancelled and not t.ignores_cancel:
            return True
        if t.sleep_until is not None:
            return t.sleep_until <= self.now
        return t.waiting_on is None or t.waiting_on.done

    def _runnable(self, finished: Optional[TaskVal] = None) -> Optional[TaskVal]:
        """Parent first: the task awaiting `finished` resumes before any other task starts; then ready tasks in
        launch order; then any other task that can run (a controlled, deterministic schedule)."""
        if finished is not None:
            for t in self.all + [self.main]:
                if t.state == "suspended" and t.waiting_on is finished and t is not self.current:
                    return t
        for t in self.all:
            if self._can_run(t):
                return t
        if self.main.state == "suspended" and self._can_run(self.main) and self.main is not self.current:
            return self.main
        if self.current.state == "suspended" and self._can_run(self.current):
            return self.current                  # only the waiting task itself can go on (its deadline has passed)
        # nothing can run: advance virtual time to the earliest sleeper (controlled scheduler)
        sleepers = [t for t in self.all + [self.main] if t.state == "suspended" and t.sleep_until is not None and not t.cancelled]
        if sleepers:
            self.now = min(t.sleep_until for t in sleepers)
            self.trace(f"time {self.now // 1_000_000}ms")
            return self._runnable()
        return None

    def _switch_to(self, t: TaskVal):
        me = self.current
        self.current = t
        if t.state == "ready":
            t.state = "running"
            self._start_thread(t)
        else:
            t.state = "running"
            self.trace(f"resume {t.name}")
        t.wake.set()
        if me.state != "done":
            me.wake.wait()
            me.wake.clear()
            self.current = me

    def _pick_next(self, finished: TaskVal):
        """A task finished: wake whoever can run next (ready tasks in launch order, then a suspended task
        whose awaited task is done, then the main task)."""
        nxt = self._runnable(finished)
        if nxt is None:
            raise RuntimeError("executor: no runnable task")
        self.current = nxt
        if nxt.state == "ready":
            nxt.state = "running"
            self._start_thread(nxt)
        else:
            nxt.state = "running"
            self.trace(f"resume {nxt.name}")
        nxt.wake.set()

    def block_until(self, target: Optional[TaskVal], until: Optional[int] = None, observe_cancel: bool = True):
        """The current task waits for `target` to complete, or for virtual time `until` (an await):
        suspend, run the other tasks, resume when the condition holds or the task is cancelled (unless
        `observe_cancel` is false: a cancelled `join` still waits for both of its futures, 12.5)."""
        me = self.current
        def satisfied():
            if target is not None:
                return target.done
            return self.now >= until
        me.ignores_cancel = not observe_cancel
        try:
            self._wait(me, target, until, satisfied, observe_cancel)
        finally:
            me.ignores_cancel = False

    def _wait(self, me, target, until, satisfied, observe_cancel):
        while not satisfied():
            if me.cancelled and observe_cancel:
                raise CancelSignal()
            me.state = "suspended"
            me.waiting_on = target
            me.sleep_until = until
            self.trace(f"suspend {me.name}")
            nxt = self._runnable()
            if nxt is None or nxt is me:
                if target is not None and target.state == "ready":
                    nxt = target
                elif nxt is me:
                    me.state, me.waiting_on, me.sleep_until = "running", None, None
                    continue
                else:
                    raise RuntimeError(f"executor: {me.name} waits for something that cannot progress")
            self._switch_to(nxt)
            me.waiting_on, me.sleep_until = None, None
            me.state = "running"
            if me.cancelled and observe_cancel:
                raise CancelSignal()

    # ---- cancellation and joins -----------------------------------------------------
    def cancel(self, t: TaskVal):
        if not t.done and not t.cancelled:
            t.cancelled = True
            self.trace(f"cancel {t.name}")

    def join(self, t: TaskVal):
        """Run `t` to completion from the current task (a scope exit or an await)."""
        if not t.done:
            self.block_until(t)
        self.trace(f"join {t.name}")
        if t.error is not None:
            raise t.error

    def detach(self, t: TaskVal):
        if len(self.detached) >= SCOPE_BOUND:
            raise DefectSignal("out_of_memory")      # the detached-task queue has a declared bound (PROG-NFR-13)
        self.detached.append(t)

    def drain_detached(self):
        for t in list(self.detached):
            if not t.done:
                self.block_until(t)
