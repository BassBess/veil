"""Static execution and content-free per-request diagnostics."""
from dataclasses import asdict, dataclass
from pathlib import Path
import sys
from time import monotonic

from veil_engine.components import EngineError

MAX_SECONDS = 170


def memory_bytes():
    try:
        if sys.platform.startswith("linux"):
            with Path("/proc/self/status").open() as stream:
                for line in stream:
                    if line.startswith("VmRSS:"):
                        return int(line.split()[1]) * 1024
        elif sys.platform == "win32":
            import ctypes
            from ctypes import wintypes

            class Counters(ctypes.Structure):
                _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
                    (name, ctypes.c_size_t) for name in
                    ("peak", "working", "paged_peak", "paged", "nonpaged_peak",
                     "nonpaged", "pagefile", "pagefile_peak")
                ]

            counters = Counters()
            counters.cb = ctypes.sizeof(counters)
            process = ctypes.windll.kernel32.GetCurrentProcess
            process.restype = wintypes.HANDLE
            read = ctypes.windll.psapi.GetProcessMemoryInfo
            read.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
            if read(process(), ctypes.byref(counters), counters.cb):
                return counters.working
        else:
            import resource
            peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
            return peak if sys.platform == "darwin" else peak * 1024
    except (OSError, ValueError, AttributeError, ImportError):
        pass
    return None


@dataclass(frozen=True)
class Budget:
    max_latency_ms: int = MAX_SECONDS * 1000
    max_memory_mb: int = 0
    max_invocations: int = 50_000

    @classmethod
    def parse(cls, value):
        if not isinstance(value, dict) or set(value) - set(cls.__dataclass_fields__):
            raise EngineError("Invalid inference budget.")
        limits = {"max_latency_ms": (1, MAX_SECONDS * 1000), "max_memory_mb": (0, 65_536),
                  "max_invocations": (1, 50_000)}
        for key, limit in value.items():
            if type(limit) is not int or not limits[key][0] <= limit <= limits[key][1]:
                raise EngineError(f"Budget {key} must be between {limits[key][0]} and {limits[key][1]}.")
        return cls(**value)


@dataclass
class Metric:
    component: str
    capability: str
    invocations: int = 0
    failures: int = 0
    model_calls: int = 0
    latency_ms: float = 0.0
    memory_bytes: int | None = None
    decision: str = "run"


class Controller:
    def __init__(self, budget, route, cancelled=lambda: False):
        self.budget = budget
        self.route = route
        self.cancelled = cancelled
        self.start = monotonic()
        self.invocations = 0
        self.metrics = {}
        self.current = None

    def checkpoint(self):
        if self.cancelled():
            raise EngineError("Detection was cancelled.")
        if (monotonic() - self.start) * 1000 > self.budget.max_latency_ms:
            raise EngineError("Detection exceeded the profile latency budget.")
        if self.budget.max_memory_mb:
            memory = memory_bytes()
            if memory is None:
                raise EngineError("Memory measurement is unavailable for the profile budget.")
            if memory > self.budget.max_memory_mb * 1024**2:
                raise EngineError("Detection exceeded the profile memory budget.")

    def count_invocation(self):
        if self.invocations >= self.budget.max_invocations:
            raise EngineError("Detection exceeded the profile invocation budget.")
        self.invocations += 1

    def infer(self, operation, *args):
        self.checkpoint()
        self.count_invocation()
        self.current.model_calls += 1
        result = operation(*args)
        self.checkpoint()
        return result

    def invoke(self, step, handler, *args, mandatory=False):
        metric = self.metrics.setdefault(step, Metric(step.component, step.capability))
        start = monotonic()
        try:
            self.checkpoint()
            if not mandatory and not self.route(step):
                metric.decision = "declined"
                # Only static, complete runs may publish results in Phase 1.
                raise EngineError("The selected routing policy did not complete detection.")
            self.count_invocation()
            metric.invocations += 1
            self.current = metric
            result = handler(*args)
            self.checkpoint()
            return result
        except Exception:
            metric.failures += 1
            if metric.decision != "declined":
                metric.decision = "failed"
            raise
        finally:
            self.current = None
            metric.latency_ms += round((monotonic() - start) * 1000, 2)
            memory = memory_bytes()
            if memory is not None:
                metric.memory_bytes = max(memory, metric.memory_bytes or 0)

    def report(self):
        return [asdict(metric) for metric in self.metrics.values()]
