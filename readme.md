# torch.market

ProgramID: 8hbUkonssSEEtkqzwM7ZcZrD9evacM92TcWSooVF4BeT

- read the [whitepaper](./docs/whitepaper.md).
- read how the engine handles [risk](./docs/risk.md).
- 71/71 passing kani proofs in [verification](./docs/verification.md).
- internal [audit](./docs/audit.md).
- develop on torch and use the test suite with the [sdk](./docs/sdk.md).

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
