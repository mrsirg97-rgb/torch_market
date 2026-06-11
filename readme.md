# torch.market

ProgramID: E5b4rBqtS5jRvjHcYZ3ZSNo2sdSPJtauQKkEacKmmjqG

**NOTE - this is torch.market v21.0.0, not live in production, utilizing deep_pool**

- read the [whitepaper](./docs/whitepaper.md).
- read how the engine handles [risk](./docs/risk.md).
- 110/110 passing litesvm tests in [tests](./docs/litesvm.md)
- 55/55 passing prop tests in [properties](./docs/properties.md)
- 97/97 passing kani proofs in [verification](./docs/verification.md).
- internal [audit](./docs/audit.md).
- develop on torch and use the test suite with the [sdk](./packages/sdk/readme.md).
- deep_pool [integration](./docs/deeppool.md).

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

gcloud services enable compute.googleapis.com run.googleapis.com sqladmin.googleapis.com artifactregistry.googleapis.com secretmanager.googleapis.com dns.googleapis.com