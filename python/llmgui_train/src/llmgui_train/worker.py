"""stdio JSON-RPC worker launched by the Rust CLI (``python -m llmgui_train.worker``).

Protocol (newline delimited JSON, one object per line):

* request:  ``{"jsonrpc": "2.0", "id": <str|int>, "method": <str>, "params": {...}}``
* response: ``{"jsonrpc": "2.0", "id": ..., "result": {...}}`` or ``... "error": {...}``
* notification: ``{"jsonrpc": "2.0", "method": "train.progress"|"done", ...}``

Supported methods: ``hello``, ``ping``, ``estimate``, ``train.start``,
``train.cancel``, ``export.merge``, ``export.gguf``.

The worker is dependency free at import time; ``train.start`` reports a
structured ``missing_dependency`` error when torch/transformers/peft are absent.
"""

from __future__ import annotations

import argparse
import queue
import sys
import threading
import traceback
from collections.abc import Callable, Iterable
from typing import Any, TextIO

from . import PROTOCOL_VERSION, __version__, capabilities, protocol, trainer
from . import estimate as estimate_module
from . import export as export_module
from .config import ConfigError, TrainConfig, load_train_config, parse_config

TrainRunner = Callable[..., dict[str, Any]]


class Worker:
    """Line oriented JSON-RPC worker.

    The class is usable in-process (inject ``stdin``/``stdout``/``train_runner``
    in tests) and as the real subprocess entry point via :func:`main`.
    """

    def __init__(
        self,
        *,
        stdin: TextIO | None = None,
        stdout: TextIO | None = None,
        stderr: TextIO | None = None,
        train_runner: TrainRunner | None = None,
        protocol_version: int = PROTOCOL_VERSION,
        enforce_dependencies: bool = True,
    ) -> None:
        self.stdin = stdin if stdin is not None else sys.stdin
        self.stdout = stdout if stdout is not None else sys.stdout
        self.stderr = stderr if stderr is not None else sys.stderr
        self.train_runner: TrainRunner = train_runner or trainer.train
        self.protocol_version = protocol_version
        self.enforce_dependencies = enforce_dependencies

        self._write_lock = threading.Lock()
        self._cancel_event = threading.Event()
        self._training_thread: threading.Thread | None = None
        self._training_job: dict[str, Any] | None = None
        self._state_lock = threading.Lock()
        self._queue: queue.Queue[tuple[str, Any]] = queue.Queue()
        self._exit_code = 0
        self._eof = False
        self._terminal = False

    # -- io -----------------------------------------------------------------
    def write_frame(self, frame: dict[str, Any]) -> None:
        line = protocol.serialize(frame)
        with self._write_lock:
            self.stdout.write(line + "\n")
            self.stdout.flush()

    def _write_result(self, request: protocol.Request, result: Any) -> None:
        self.write_frame(protocol.response(request.id, result))

    def _write_error(
        self,
        request: protocol.Request | None,
        code: int | protocol.ErrorCode,
        message: str,
        data: Any | None = None,
    ) -> None:
        request_id = request.id if request is not None else None
        self.write_frame(protocol.error_response(request_id, code, message, data))

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        self.write_frame(protocol.notification(method, params))

    # -- serving ------------------------------------------------------------
    def serve(self) -> int:
        """Read stdin until EOF or until a terminal command finished."""

        # Torch/numpy import can deadlock if a daemon thread is already blocked
        # reading stdin on Windows; warm the heavy stack before starting it.
        if self.enforce_dependencies and not trainer.missing_dependencies():
            import importlib

            for package in ("torch", "transformers", "peft"):
                importlib.import_module(package)

        reader = threading.Thread(target=self._read_loop, name="worker-stdin", daemon=True)
        reader.start()

        while True:
            kind, payload = self._queue.get()
            if kind == "line":
                self.handle_line(payload)
                if not self._is_training_alive():
                    with self._state_lock:
                        terminal = self._terminal
                    if terminal:
                        return self._exit_code
                continue
            if kind == "eof":
                self._eof = True
                return self._wait_for_training()
            if kind == "training_done":
                self._emit_training_done(payload)
                return self._exit_code

    def _read_loop(self) -> None:
        try:
            for line in self.stdin:
                self._queue.put(("line", line))
        finally:
            self._queue.put(("eof", None))

    def _wait_for_training(self) -> int:
        thread = self._training_thread
        if thread is not None and thread.is_alive():
            thread.join()
        while True:
            try:
                kind, payload = self._queue.get_nowait()
            except queue.Empty:
                break
            if kind == "training_done":
                self._emit_training_done(payload)
                break
        return self._exit_code

    def _is_training_alive(self) -> bool:
        thread = self._training_thread
        return thread is not None and thread.is_alive()

    # -- dispatch -----------------------------------------------------------
    def handle_line(self, line: str) -> dict[str, Any] | None:
        try:
            request = protocol.parse_request(line)
        except protocol.RpcError as exc:
            self.write_frame(exc.to_response(None))
            return None
        return self.handle(request)

    def handle(self, request: protocol.Request) -> dict[str, Any] | None:
        handlers: dict[str, Callable[[protocol.Request], Any]] = {
            "hello": self._handle_hello,
            "ping": self._handle_ping,
            "estimate": self._handle_estimate,
            "train.start": self._handle_train_start,
            "train.cancel": self._handle_train_cancel,
            "export.merge": self._handle_export_merge,
            "export.gguf": self._handle_export_gguf,
        }
        handler = handlers.get(request.method)
        if handler is None:
            if request.is_notification:
                return None
            self._write_error(
                request,
                protocol.ErrorCode.METHOD_NOT_FOUND,
                f"method not found: {request.method}",
                data={"method": request.method, "supported": sorted(handlers)},
            )
            return None
        try:
            result = handler(request)
        except protocol.RpcError as exc:  # pragma: no cover - handlers raise structured errors
            self._note_terminal_failure(request)
            if not request.is_notification:
                self._write_error(request, exc.code, exc.message, exc.data)
            return None
        except ConfigError as exc:
            self._note_terminal_failure(request)
            if not request.is_notification:
                self._write_error(request, protocol.ErrorCode.CONFIG_ERROR, str(exc))
            return None
        except trainer.MissingDependency as exc:
            self._note_terminal_failure(request)
            if not request.is_notification:
                self._write_error(
                    request,
                    protocol.ErrorCode.MISSING_DEPENDENCY,
                    str(exc),
                    exc.to_dict(),
                )
            return None
        except trainer.TrainingCancelled as exc:
            self._note_terminal_failure(request)
            if not request.is_notification:
                self._write_error(
                    request,
                    protocol.ErrorCode.TRAINING_CANCELLED,
                    str(exc),
                    {"code": "training_cancelled"},
                )
            return None
        except export_module.ExportError as exc:
            self._note_terminal_failure(request)
            if not request.is_notification:
                self._write_error(
                    request,
                    protocol.server_error_code(exc.code),
                    str(exc),
                    export_module.export_error_to_rpc(exc),
                )
            return None
        except Exception as exc:  # noqa: BLE001 - worker must never die on one frame
            self._note_terminal_failure(request)
            self._log_traceback()
            if not request.is_notification:
                self._write_error(
                    request,
                    protocol.ErrorCode.INTERNAL_ERROR,
                    f"{type(exc).__name__}: {exc}",
                )
            return None

        if request.is_notification:
            return None
        self._write_result(request, result)
        return result

    # -- methods ------------------------------------------------------------
    def _handle_hello(self, request: protocol.Request) -> dict[str, Any]:
        requested = request.params.get("protocol", self.protocol_version)
        if isinstance(requested, bool) or not isinstance(requested, int):
            raise protocol.RpcError(
                protocol.ErrorCode.INVALID_PARAMS,
                f"params.protocol must be an integer, got {requested!r}",
            )
        if requested > self.protocol_version or requested < 1:
            raise protocol.RpcError(
                protocol.ErrorCode.UNSUPPORTED_PROTOCOL,
                f"unsupported protocol version {requested}; supported: {self.protocol_version}",
                data={"protocol": self.protocol_version},
            )
        report = capabilities.detect()
        return {
            "event": "hello_ok",
            "protocol": self.protocol_version,
            "server": "llmgui_train",
            "version": __version__,
            "capabilities": report,
            "training": {
                "ready": report["training"]["ready"],
                "missing": report["training"]["missing"],
            },
        }

    def _handle_ping(self, request: protocol.Request) -> dict[str, Any]:
        return {"pong": True, "protocol": self.protocol_version}

    def _handle_estimate(self, request: protocol.Request) -> dict[str, Any]:
        params = dict(request.params)
        if "config" in params:
            config_payload = params.pop("config")
            if not isinstance(config_payload, dict):
                raise protocol.RpcError(
                    protocol.ErrorCode.INVALID_PARAMS,
                    "params.config must be an object",
                )
            config = parse_config(config_payload)
            estimate = estimate_module.estimate_from_config(config)
            if params:
                raise protocol.RpcError(
                    protocol.ErrorCode.INVALID_PARAMS,
                    "cannot combine params.config with other estimate parameters",
                )
            return estimate
        try:
            return estimate_module.estimate_from_mapping(params)
        except estimate_module.EstimateError as exc:
            raise protocol.RpcError(protocol.ErrorCode.INVALID_PARAMS, str(exc)) from exc

    def _load_config(self, params: dict[str, Any]) -> TrainConfig:
        config_path = params.get("config_path")
        inline = params.get("config")
        if config_path is not None and inline is not None:
            raise protocol.RpcError(
                protocol.ErrorCode.INVALID_PARAMS,
                "provide either config_path or config, not both",
            )
        if config_path is not None:
            if not isinstance(config_path, str) or not config_path.strip():
                raise protocol.RpcError(
                    protocol.ErrorCode.INVALID_PARAMS, "config_path must be a non-empty string"
                )
            return load_train_config(config_path, check_paths=True)
        if inline is not None:
            if not isinstance(inline, dict):
                raise protocol.RpcError(
                    protocol.ErrorCode.INVALID_PARAMS, "config must be an object"
                )
            return parse_config(inline, check_paths=True)
        raise protocol.RpcError(
            protocol.ErrorCode.INVALID_PARAMS,
            "train.start requires 'config_path' or 'config'",
        )

    def _handle_train_start(self, request: protocol.Request) -> dict[str, Any]:
        # train.start is a terminal command: the CLI breaks out of its read loop
        # on the final done/error frame and then waits for this process to exit.
        with self._state_lock:
            self._terminal = True
            if self._is_training_alive():
                raise protocol.RpcError(
                    protocol.ErrorCode.INVALID_REQUEST,
                    "a training job is already running",
                )
        config = self._load_config(dict(request.params))

        missing = trainer.missing_dependencies() if self.enforce_dependencies else []
        if missing:
            raise trainer.MissingDependency(missing)

        # Import the heavy stack in the main worker thread before starting the
        # training thread. On Windows, importing torch from a newly spawned
        # thread while the main thread blocks on a queue can deadlock.
        if self.enforce_dependencies:
            import importlib

            for package in ("torch", "transformers", "peft"):
                importlib.import_module(package)

        cancel_event = threading.Event()
        with self._state_lock:
            self._cancel_event = cancel_event
            self._training_job = {"config": config, "started": True}

        def runner() -> None:
            try:
                result = self.train_runner(
                    config,
                    on_event=self._on_train_event,
                    cancel_event=cancel_event,
                )
            except trainer.TrainingCancelled as exc:
                payload: dict[str, Any] = {
                    "ok": False,
                    "cancelled": True,
                    "error": {"code": "training_cancelled", "message": str(exc)},
                }
                self.notify("done", payload)
                self._queue.put(("training_done", None))
                return
            except trainer.MissingDependency as exc:
                self._write_frame_notification_error(exc)
                self._queue.put(("training_done", None))
                return
            except Exception as exc:  # noqa: BLE001 - convert to protocol error
                self._log_traceback()
                self.notify(
                    "error",
                    {
                        "code": "training_failed",
                        "message": f"{type(exc).__name__}: {exc}",
                    },
                )
                self._queue.put(("training_done", None))
                return
            self.notify("done", {"ok": True, "result": result})
            self._queue.put(("training_done", None))

        thread = threading.Thread(target=runner, name="worker-train", daemon=True)
        with self._state_lock:
            self._training_thread = thread
        thread.start()
        return {
            "started": True,
            "adapter_dir": config.output_dir,
            "method": config.method,
            "base_model": config.base_model,
        }

    def _write_frame_notification_error(self, exc: trainer.MissingDependency) -> None:
        self.notify(
            "error",
            {
                "code": "missing_dependency",
                "message": str(exc),
                "missing": exc.missing,
            },
        )

    def _on_train_event(self, event: dict[str, Any]) -> None:
        kind = str(event.get("kind", "train.progress"))
        params = {key: value for key, value in event.items() if key != "kind"}
        self.notify(kind, params)

    def _emit_training_done(self, payload: Any) -> None:
        with self._state_lock:
            self._training_job = None

    def _handle_train_cancel(self, request: protocol.Request) -> dict[str, Any]:
        running = self._is_training_alive()
        if not running:
            return {"cancelled": False, "running": False}
        self._cancel_event.set()
        return {"cancelled": True, "running": True}

    def _handle_export_merge(self, request: protocol.Request) -> dict[str, Any]:
        params = dict(request.params)
        for key in ("base_model", "adapter_dir", "output_path"):
            if not isinstance(params.get(key), str) or not params[key].strip():
                raise protocol.RpcError(
                    protocol.ErrorCode.INVALID_PARAMS,
                    f"export.merge requires a non-empty string '{key}'",
                )
        result = export_module.merge_adapter(
            params["base_model"],
            params["adapter_dir"],
            params["output_path"],
            tool=params.get("tool"),
            timeout=float(params.get("timeout", 3600.0)),
            extra_args=params.get("extra_args", ()),
        )
        if not result.get("ok"):
            raise protocol.RpcError(
                protocol.server_error_code(result["error"].get("code", "export_failed")),
                result["error"].get("message", "merge failed"),
                data=result["error"],
            )
        return result

    def _handle_export_gguf(self, request: protocol.Request) -> dict[str, Any]:
        params = dict(request.params)
        for key in ("model_dir", "output_path"):
            if not isinstance(params.get(key), str) or not params[key].strip():
                raise protocol.RpcError(
                    protocol.ErrorCode.INVALID_PARAMS,
                    f"export.gguf requires a non-empty string '{key}'",
                )
        result = export_module.export_gguf(
            params["model_dir"],
            params["output_path"],
            outtype=str(params.get("outtype", "f16")),
            tool=params.get("tool"),
            timeout=float(params.get("timeout", 3600.0)),
            extra_args=params.get("extra_args", ()),
        )
        if not result.get("ok"):
            raise protocol.RpcError(
                protocol.server_error_code(result["error"].get("code", "export_failed")),
                result["error"].get("message", "GGUF export failed"),
                data=result["error"],
            )
        return result

    # -- diagnostics --------------------------------------------------------
    def _note_terminal_failure(self, request: protocol.Request) -> None:
        """A failed terminal command must be visible in the process exit code."""

        if request.method == "train.start":
            self._exit_code = 1

    def _log_traceback(self) -> None:
        try:
            self.stderr.write(traceback.format_exc())
            self.stderr.flush()
        except Exception:  # pragma: no cover - stderr always best effort
            pass


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="llmgui_train.worker",
        description="llmgui training worker (stdio NDJSON JSON-RPC)",
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"llmgui-train {__version__} (protocol {PROTOCOL_VERSION})",
    )
    parser.add_argument(
        "--allow-missing-deps",
        action="store_true",
        help="use a stub trainer when the training stack is missing (debug helper)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="alias for --allow-missing-deps: never run real training",
    )
    args = parser.parse_args(list(argv) if argv is not None else None)

    runner: TrainRunner | None = None
    if args.allow_missing_deps or args.dry_run:

        def stub_runner(config: TrainConfig, **_: Any) -> dict[str, Any]:
            return {
                "ok": True,
                "stub": True,
                "adapter_dir": config.output_dir,
                "steps": 0,
                "metrics": {},
            }

        runner = stub_runner

    worker = Worker(
        train_runner=runner,
        enforce_dependencies=runner is None,
    )
    try:
        return worker.serve()
    except KeyboardInterrupt:  # pragma: no cover - interactive only
        return 130
    except Exception:  # pragma: no cover - top level guard
        traceback.print_exc(file=sys.stderr)
        return 1


if __name__ == "__main__":  # pragma: no cover - module entry point
    raise SystemExit(main())


__all__ = ["Worker", "main"]
