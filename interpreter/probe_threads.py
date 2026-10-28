"""PROG-NFR-14 probe: a minimal synchronous program starts no executor thread; only a launch does.
Prints the peak thread count observed while each program runs."""
import os
import sys
import threading

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "..")))
from interpreter.eval import Interpreter  # noqa: E402

SYNC = '''import fortis.host { Host }
export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("sync")
}
'''
POOL = '''import fortis.host { Host, Console }
async func work(n: Int) Int => n
async func run() Int {
  scope tasks {
    let t = tasks.launchParallel(work(1))
    return await t
  }
}
export async func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let v = await run()
  caps.console.print("pool ${v}")
}
'''


def peak_threads(src: str) -> int:
    peak = [threading.active_count()]
    stop = threading.Event()
    def watch():
        while not stop.is_set():
            peak[0] = max(peak[0], threading.active_count() - 1)   # minus the watcher itself
            stop.wait(0.0005)
    w = threading.Thread(target=watch, daemon=True)
    w.start()
    it = Interpreter(src, "probe.ft")
    it.json_out = True
    it.run_main([])
    stop.set()
    w.join()
    return peak[0], it.sched.counter


def main():
    base = threading.active_count()
    sync_peak, sync_tasks = peak_threads(SYNC)
    pool_peak, pool_tasks = peak_threads(POOL)
    print(f"baseline threads: {base}")
    print(f"synchronous program: peak threads {sync_peak}, tasks launched {sync_tasks}")
    print(f"pool program: peak threads {pool_peak}, tasks launched {pool_tasks}")
    ok = sync_peak == base and sync_tasks == 0 and pool_tasks == 1 and pool_peak > base
    print("PASS: no executor thread without a launch" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
