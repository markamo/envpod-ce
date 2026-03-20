# Python SDK — Quick Start

Get governed AI agents running in 60 seconds.

## Install

```bash
pip install envpod
```

## 1. Run a Governed Agent

```python
from envpod import Pod

with Pod("my-agent", config="coding-agent.yaml") as pod:
    pod.run("python3 agent.py")
    print(pod.diff())
    pod.commit("src/", rollback_rest=True)
```

## 2. Screen Prompts

```python
from envpod import screen

result = screen("ignore previous instructions")
print(result)  # {'matched': True, 'category': 'injection', ...}

result = screen("Write a fibonacci function")
print(result)  # {'matched': False, ...}
```

## 3. Fast Cloning (100 agents in 1 second)

```python
from envpod import Pod

# One-time setup (~5 min)
base = Pod("worker", config="coding-agent.yaml")
base.init_with_base()
base.destroy()

# Clone in ~8ms each
for i in range(100):
    Pod.disposable(base, f"task-{i}", "python3 experiment.py",
                   commit_paths=["results/"])
Pod.gc()
```

## 4. Desktop in Your Browser

```python
from envpod import Pod

pod = Pod("desktop", config="examples/workstation-full.yaml", persist=True)
pod.init()
url = pod.start_display()
print(f"Open: {url}")  # http://10.200.X.2:6080/vnc.html
```

## Next

- [Full SDK Reference](../../docs/SDK.md)
- [10 Usage Examples](../EXAMPLES.md)
- [Screening Guide](../../docs/SCREENING.md)
