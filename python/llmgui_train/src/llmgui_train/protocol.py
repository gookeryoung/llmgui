"""JSON-RPC 2.0 style framing for the llmgui train worker.

The Rust CLI launches ``python -m llmgui_train.worker`` and exchanges one JSON
object per line over stdin/stdout.  This module is dependency free on purpose:
it only validates frames and builds response objects, so it can be imported
(and tested) without torch/transformers installed.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from enum import IntEnum
from typing import Any

JSONRPC_VERSION = "2.0"


class ErrorCode(IntEnum):
    """JSON-RPC reserved codes plus the worker specific server errors."""

    # Reserved by JSON-RPC 2.0.
    PARSE_ERROR = -32700
    INVALID_REQUEST = -32600
    METHOD_NOT_FOUND = -32601
    INVALID_PARAMS = -32602
    INTERNAL_ERROR = -32603

    # Implementation defined server errors (-32000 .. -32099).
    MISSING_DEPENDENCY = -32010
    CONFIG_ERROR = -32011
    TRAINING_FAILED = -32012
    TRAINING_CANCELLED = -32013
    EXPORT_FAILED = -32014
    TOOL_NOT_FOUND = -32015
    DATA_ERROR = -32016
    IO_ERROR = -32017
    UNSUPPORTED_PROTOCOL = -32018


class RpcError(Exception):
    """An error that can be serialised as a JSON-RPC error object."""

    def __init__(
        self,
        code: int | ErrorCode,
        message: str,
        data: Any | None = None,
    ) -> None:
        super().__init__(message)
        self.code = int(code)
        self.message = message
        self.data = data

    def to_error(self) -> dict[str, Any]:
        error: dict[str, Any] = {"code": self.code, "message": self.message}
        if self.data is not None:
            error["data"] = self.data
        return error

    def to_response(self, request_id: Any = None) -> dict[str, Any]:
        return error_response(request_id, self.code, self.message, self.data)

    def __repr__(self) -> str:  # pragma: no cover - debugging helper
        return f"RpcError(code={self.code!r}, message={self.message!r})"


@dataclass(frozen=True)
class Request:
    """A validated JSON-RPC request frame."""

    method: str
    params: dict[str, Any] = field(default_factory=dict)
    id: Any = None
    has_id: bool = False
    raw: dict[str, Any] = field(default_factory=dict)

    @property
    def is_notification(self) -> bool:
        """True when the frame has no ``id`` and must not get a response."""

        return not self.has_id


def parse_frame(payload: str | bytes) -> dict[str, Any]:
    """Parse one NDJSON frame into a JSON object.

    Raises :class:`RpcError` with ``PARSE_ERROR`` when the payload is not valid
    JSON and ``INVALID_REQUEST`` when it is valid JSON but not an object.
    """

    if isinstance(payload, bytes):
        try:
            text = payload.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise RpcError(ErrorCode.PARSE_ERROR, f"frame is not valid UTF-8: {exc}") from exc
    else:
        text = payload

    stripped = text.strip()
    if not stripped:
        raise RpcError(ErrorCode.PARSE_ERROR, "empty frame: expected one JSON object")

    try:
        obj = json.loads(stripped)
    except json.JSONDecodeError as exc:
        raise RpcError(
            ErrorCode.PARSE_ERROR,
            f"invalid JSON: {exc.msg}",
            data={"line": text.rstrip("\r\n"), "position": exc.pos},
        ) from exc

    if not isinstance(obj, dict):
        raise RpcError(
            ErrorCode.INVALID_REQUEST,
            f"frame must be a JSON object, got {type(obj).__name__}",
        )
    return obj


def validate_request(obj: dict[str, Any]) -> Request:
    """Validate a decoded JSON object and turn it into a :class:`Request`."""

    if "jsonrpc" in obj and obj["jsonrpc"] != JSONRPC_VERSION:
        raise RpcError(
            ErrorCode.INVALID_REQUEST,
            f"unsupported jsonrpc version: {obj['jsonrpc']!r}",
        )

    method = obj.get("method")
    if not isinstance(method, str) or not method:
        raise RpcError(ErrorCode.INVALID_REQUEST, "request is missing a non-empty string 'method'")

    params = obj.get("params", {})
    if params is None:
        params = {}
    if not isinstance(params, dict):
        raise RpcError(
            ErrorCode.INVALID_PARAMS,
            f"params must be a JSON object, got {type(params).__name__}",
        )

    has_id = "id" in obj
    request_id = obj.get("id")
    if has_id and not (request_id is None or isinstance(request_id, (str, int))):
        raise RpcError(ErrorCode.INVALID_REQUEST, "id must be a string, integer or null")

    return Request(
        method=method,
        params=params,
        id=request_id,
        has_id=has_id,
        raw=obj,
    )


def parse_request(payload: str | bytes) -> Request:
    """Parse and validate a single NDJSON frame."""

    return validate_request(parse_frame(payload))


def response(request_id: Any, result: Any) -> dict[str, Any]:
    return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "result": result}


def error_response(
    request_id: Any,
    code: int | ErrorCode,
    message: str,
    data: Any | None = None,
) -> dict[str, Any]:
    error: dict[str, Any] = {"code": int(code), "message": message}
    if data is not None:
        error["data"] = data
    return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "error": error}


def notification(method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    frame: dict[str, Any] = {"jsonrpc": JSONRPC_VERSION, "method": method}
    if params is not None:
        frame["params"] = params
    return frame


def serialize(frame: dict[str, Any]) -> str:
    """Serialise one frame as a single compact line (no trailing newline)."""

    return json.dumps(frame, ensure_ascii=False, separators=(",", ":"), default=str)


def server_error_code(name: str) -> ErrorCode:
    """Map a structured error name to its server error code."""

    mapping = {
        "missing_dependency": ErrorCode.MISSING_DEPENDENCY,
        "config_error": ErrorCode.CONFIG_ERROR,
        "training_failed": ErrorCode.TRAINING_FAILED,
        "training_cancelled": ErrorCode.TRAINING_CANCELLED,
        "export_failed": ErrorCode.EXPORT_FAILED,
        "tool_not_found": ErrorCode.TOOL_NOT_FOUND,
        "data_error": ErrorCode.DATA_ERROR,
        "io_error": ErrorCode.IO_ERROR,
    }
    return mapping.get(name, ErrorCode.INTERNAL_ERROR)
