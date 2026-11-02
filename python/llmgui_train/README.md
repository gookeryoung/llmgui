# llmgui-train

Python training sidecar for the llmgui Rust CLI.  It talks newline-delimited
JSON (JSON-RPC 2.0 style) over stdin/stdout and provides:

| module | purpose |
|---|---|
| `config` | JSON/YAML training config loading + validation (`TrainConfig`) |
| `capabilities` | torch / CUDA / ROCm / NPU / MUSA / triton / bnb / liger detection |
| `data` | Alpaca / ShareGPT / OpenAI-chat JSONL parsing, assistant masks, sequence packing |
| `estimate` | `M = weights + lora + activations + logits + context` memory estimate |
| `export` | PEFT adapter + `training_manifest.json`; llama.cpp merge / GGUF wrappers |
| `protocol` | JSON-RPC frame parsing, serialisation and error codes |
| `worker` | `python -m llmgui_train.worker` stdio worker |
| `trainer` | lazy-import minimal QLoRA/LoRA SFT loop |
| `cli` | `llmgui-train hello|estimate|train|merge|export-gguf|worker` |

## Dependency free by default

`torch`, `transformers`, `peft`, `trl` and `bitsandbytes` are **only** imported
inside `trainer.train()`.  Without them the worker answers `train.start` with a
structured `missing_dependency` error and everything else (capabilities, config
validation, estimate, export manifests, protocol) keeps working.

## Install

```bash
# from the repository root
pip install -e python/llmgui_train            # core, no ML deps
pip install -e "python/llmgui_train[cuda]"    # torch + transformers + peft + trl + bnb
```

## Usage

```bash
python -m llmgui_train hello
python -m llmgui_train estimate --model 7b --method qlora --seq-len 2048
python -m llmgui_train train --config train.json
python -m llmgui_train worker          # what the Rust CLI spawns
```

The worker protocol:

```jsonc
{"jsonrpc":"2.0","id":"1","method":"hello","params":{"protocol":1}}
{"jsonrpc":"2.0","id":"2","method":"estimate","params":{"model":"7b","method":"qlora"}}
{"jsonrpc":"2.0","id":"3","method":"train.start","params":{"config_path":"train.json"}}
{"jsonrpc":"2.0","id":"4","method":"train.cancel"}
{"jsonrpc":"2.0","id":"5","method":"export.merge","params":{"base_model":"...","adapter_dir":"...","output_path":"..."}}
{"jsonrpc":"2.0","id":"6","method":"export.gguf","params":{"model_dir":"...","output_path":"..."}}
```

Progress is streamed as notifications (`train.progress`) and the final frame is a
`done` (or `error`) notification, which is what the Rust CLI waits for.
