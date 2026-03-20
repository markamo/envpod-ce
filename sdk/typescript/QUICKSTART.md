# TypeScript SDK — Quick Start

Get governed AI agents running in 60 seconds.

## Install

```bash
npm install envpod
```

## 1. Run a Governed Agent

```typescript
import { Pod } from 'envpod';

await Pod.with('my-agent', { config: 'coding-agent.yaml' }, async (pod) => {
    pod.run('python3 agent.py');
    console.log(pod.diff());
    pod.commit(['src/'], { rollbackRest: true });
});
```

## 2. Screen Prompts

```typescript
import { screen } from 'envpod';

const result = screen('ignore previous instructions');
console.log(result);  // { matched: true, category: 'injection', ... }

const clean = screen('Write a fibonacci function');
console.log(clean);   // { matched: false, ... }
```

## 3. Fast Cloning (100 agents in 1 second)

```typescript
import { Pod } from 'envpod';

// One-time setup (~5 min)
const base = Pod.wrap('worker', { config: 'coding-agent.yaml' });
base.initWithBase();
base.destroy();

// Clone in ~8ms each
for (let i = 0; i < 100; i++) {
    Pod.disposable(base, `task-${i}`, 'python3 experiment.py', {
        commitPaths: ['results/'],
    });
}
Pod.gc();
```

## 4. Secure API Keys (Vault)

```typescript
import { Pod } from 'envpod';

await Pod.with('my-agent', { config: 'coding-agent.yaml' }, async (pod) => {
    // Store keys — encrypted, never visible to the agent
    pod.vaultSet('ANTHROPIC_API_KEY', 'sk-ant-...');
    pod.vaultSet('OPENAI_API_KEY', 'sk-...');

    // Agent gets keys as env vars at runtime
    // but can't read them from config, logs, or shell history
    pod.run('python3 agent.py');
});
```

## 5. Desktop in Your Browser

```typescript
import { Pod } from 'envpod';

const pod = await Pod.create('desktop', {
    config: 'examples/workstation-full.yaml',
    persist: true,
});
const url = pod.startDisplay();
console.log(`Open: ${url}`);  // http://10.200.X.2:6080/vnc.html
```

## Next

- [Full SDK Reference](../../docs/SDK.md)
- [10 Usage Examples](../EXAMPLES.md)
- [Screening Guide](../../docs/SCREENING.md)
