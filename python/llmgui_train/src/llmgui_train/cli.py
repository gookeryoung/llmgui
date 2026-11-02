"""Command line entry point for ``llmgui-train``.

Subcommands mirror what the Rust CLI uses through the worker, which makes the
Python package usable standalone:

    llmgui-train hello
    llmgui-train estimate --model 7b --method qlora --seq-len 2048
    llmgui-train train --config train.json
    llmgui-train export-gguf --model-dir outputs/train --output model.gguf
    llmgui-train worker
"""

from __future__ import annotations

import argparse
import json
import shlex
import sys
from collections.abc import Sequence
from typing import Any

from . import PROTOCOL_VERSION, __version__, capabilities
from . import estimate as estimate_module
from . import export as export_module
from .config import ConfigError, load_train_config
from .worker import main as worker_main


def _dump(payload: Any) -> None:
    print(json.dumps(payload, indent=2, ensure_ascii=False))


def cmd_hello(_args: argparse.Namespace) -> int:
    _dump(
        {
            "server": "llmgui_train",
            "version": __version__,
            "protocol": PROTOCOL_VERSION,
            "capabilities": capabilities.detect(),
        }
    )
    return 0


def cmd_capabilities(_args: argparse.Namespace) -> int:
    _dump(capabilities.detect())
    return 0


def cmd_estimate(args: argparse.Namespace) -> int:
    estimate = estimate_module.estimate_memory(
        args.model,
        method=args.method,
        bits=args.bits,
        seq_len=args.seq_len,
        micro_batch=args.micro_batch,
        grad_accum=args.grad_accum,
        lora_r=args.lora_r,
        gradient_checkpointing=not args.no_gradient_checkpointing,
        total_budget_gb=args.budget_gb,
    )
    if args.json:
        _dump(estimate)
    else:
        print(estimate_module.format_report(estimate))
    return 0


def cmd_train(args: argparse.Namespace) -> int:
    try:
        config = load_train_config(args.config, check_paths=True)
    except ConfigError as exc:
        print(f"config error: {exc}", file=sys.stderr)
        return 2
    from . import trainer

    try:
        result = trainer.train(config)
    except trainer.MissingDependency as exc:
        print(json.dumps(exc.to_dict(), indent=2, ensure_ascii=False), file=sys.stderr)
        return 3
    _dump(result)
    return 0


def cmd_export_gguf(args: argparse.Namespace) -> int:
    try:
        result = export_module.export_gguf(
            args.model_dir,
            args.output,
            outtype=args.outtype,
            tool=shlex.split(args.tool) if args.tool else None,
        )
    except export_module.ExportError as exc:
        _dump(exc.to_dict(), )
        return 2
    _dump(result)
    return 0 if result.get("ok") else 1


def cmd_merge(args: argparse.Namespace) -> int:
    try:
        if getattr(args, "native", False):
            result = export_module.merge_adapter_peft(
                args.base_model,
                args.adapter_dir,
                args.output,
            )
        else:
            result = export_module.merge_adapter(
                args.base_model,
                args.adapter_dir,
                args.output,
                tool=shlex.split(args.tool) if args.tool else None,
            )
    except export_module.ExportError as exc:
        _dump(exc.to_dict())
        return 2
    _dump(result)
    return 0 if result.get("ok") else 1


def cmd_worker(args: argparse.Namespace) -> int:
    extra = ["--dry-run"] if args.dry_run else []
    return worker_main(extra)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="llmgui-train",
        description="llmgui Python training toolkit (QLoRA/LoRA SFT, export, memory estimate)",
    )
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("hello", help="print version and capability report").set_defaults(
        func=cmd_hello
    )
    sub.add_parser("capabilities", help="print the capability report").set_defaults(
        func=cmd_capabilities
    )

    estimate = sub.add_parser("estimate", help="estimate training memory")
    estimate.add_argument("--model", default="7b", help="7b, 14b, 32b, or another size")
    estimate.add_argument("--method", default="qlora", choices=["qlora", "lora", "full"])
    estimate.add_argument("--bits", type=int, default=4, choices=[4, 8])
    estimate.add_argument("--seq-len", type=int, default=2048)
    estimate.add_argument("--micro-batch", type=int, default=1)
    estimate.add_argument("--grad-accum", type=int, default=8)
    estimate.add_argument("--lora-r", type=int, default=16)
    estimate.add_argument("--no-gradient-checkpointing", action="store_true")
    estimate.add_argument("--budget-gb", type=float, default=24.0)
    estimate.add_argument("--json", action="store_true", help="emit JSON instead of text")
    estimate.set_defaults(func=cmd_estimate)

    train = sub.add_parser("train", help="run SFT training from a config file")
    train.add_argument("--config", required=True, help="path to a JSON/YAML training config")
    train.set_defaults(func=cmd_train)

    gguf = sub.add_parser("export-gguf", help="convert a model directory to GGUF")
    gguf.add_argument("--model-dir", required=True)
    gguf.add_argument("--output", required=True)
    gguf.add_argument("--outtype", default="f16")
    gguf.add_argument("--tool", default=None, help="path to convert_hf_to_gguf.py")
    gguf.set_defaults(func=cmd_export_gguf)

    merge = sub.add_parser("merge", help="merge a LoRA adapter into a base model")
    merge.add_argument("--base-model", required=True)
    merge.add_argument("--adapter-dir", required=True)
    merge.add_argument("--output", required=True)
    merge.add_argument("--tool", default=None, help="path to llama-export-lora")
    merge.add_argument(
        "--native",
        action="store_true",
        help="use PEFT merge_and_unload instead of the external GGUF LoRA tool",
    )
    merge.set_defaults(func=cmd_merge)

    worker = sub.add_parser("worker", help="run the stdio NDJSON JSON-RPC worker")
    worker.add_argument("--dry-run", action="store_true")
    worker.set_defaults(func=cmd_worker)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    # External tools (tqdm/llama.cpp) may emit U+FFFD on Windows consoles with
    # a GBK code page; never let JSON reporting crash after a successful export.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    parser = build_parser()
    args = parser.parse_args(list(argv) if argv is not None else None)
    try:
        return int(args.func(args))
    except estimate_module.EstimateError as exc:
        print(f"estimate error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":  # pragma: no cover - module entry point
    raise SystemExit(main())
