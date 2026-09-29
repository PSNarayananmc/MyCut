#!/usr/bin/env bash
# MyCut benchmark + verification harness.
# Runs lint, tests, the headless E2E (normal + memory-constrained), and
# prints JSON evidence. Run from the repo root:
#   ./scripts/bench.sh
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

echo "== cargo test (workspace, headless members) =="
cargo test --workspace --quiet

echo "== headless E2E (normal) =="
rm -rf target/e2e
python3 - <<'PY'
import subprocess, resource, time, json
t0 = time.time()
p = subprocess.run(["./target/debug/mycut-cli", "e2e", "target/e2e"], capture_output=True, text=True)
u = resource.getrusage(resource.RUSAGE_CHILDREN)
print(json.dumps({
    "e2e": {"exit": p.returncode, "wall_s": round(time.time() - t0, 2),
            "peak_child_rss_mb": round(u.ru_maxrss / 1024, 1),
            "passed": "E2E PASSED" in p.stdout}
}, indent=2))
PY

echo "== headless E2E (constrained: RLIMIT_AS 2.5 GB) =="
rm -rf target/e2e-c
python3 - <<'PY'
import subprocess, resource, time, json
def preexec():
    resource.setrlimit(resource.RLIMIT_AS, (2_500_000_000, 2_500_000_000))
t0 = time.time()
p = subprocess.run(["./target/debug/mycut-cli", "e2e", "target/e2e-c"], capture_output=True, text=True, preexec_fn=preexec)
u = resource.getrusage(resource.RUSAGE_CHILDREN)
print(json.dumps({
    "e2e_constrained": {"exit": p.returncode, "wall_s": round(time.time() - t0, 2),
                        "peak_child_rss_mb": round(u.ru_maxrss / 1024, 1),
                        "passed": "E2E PASSED" in p.stdout}
}, indent=2))
PY

echo "bench complete"
