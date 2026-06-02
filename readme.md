# torch.market

ProgramID: E5b4rBqtS5jRvjHcYZ3ZSNo2sdSPJtauQKkEacKmmjqG

**NOTE - this is torch.market 20.0.0 next, not live in production, utilizing deep_pool**

- read the [whitepaper](./docs/whitepaper.md).
- read how the engine handles [risk](./docs/risk.md).
- 75/75 passing kani proofs in [verification](./docs/verification.md).
- internal [audit](./docs/audit.md).
- develop on torch and use the test suite with the [sdk](./docs/sdk.md).
- deep_pool [integration](./docs/deeppool_integration.md).

## run kani proofs

```bash
anchor build
cargo kani
```

## run proptest

```bash
anchor build
cargo test
```

## run the sim

```bash
python3 sim/torch_sim.py
```

Brightside Solutions, 2026
