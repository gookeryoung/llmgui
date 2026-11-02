"""Tests for :mod:`llmgui_train.protocol`."""

from __future__ import annotations

import json

import pytest

from llmgui_train import protocol
from llmgui_train.protocol import ErrorCode, Request, RpcError


def test_parse_frame_returns_object():
    frame = protocol.parse_frame('{"jsonrpc": "2.0", "id": 1, "method": "hello"}')
    assert frame == {"jsonrpc": "2.0", "id": 1, "method": "hello"}


def test_parse_frame_accepts_bytes():
    frame = protocol.parse_frame(b'{"method": "ping"}')
    assert frame["method"] == "ping"


def test_parse_frame_invalid_json():
    with pytest.raises(RpcError) as info:
        protocol.parse_frame("{not json")
    assert info.value.code == ErrorCode.PARSE_ERROR
    assert "invalid JSON" in info.value.message


def test_parse_frame_rejects_json_array():
    with pytest.raises(RpcError) as info:
        protocol.parse_frame("[1, 2, 3]")
    assert info.value.code == ErrorCode.INVALID_REQUEST


def test_parse_frame_rejects_empty_and_multiple_values():
    with pytest.raises(RpcError):
        protocol.parse_frame("   ")
    with pytest.raises(RpcError):
        protocol.parse_frame('{"a": 1} {"b": 2}')


def test_validate_request_requires_method():
    with pytest.raises(RpcError) as info:
        protocol.validate_request({"jsonrpc": "2.0", "id": 1})
    assert info.value.code == ErrorCode.INVALID_REQUEST


def test_validate_request_rejects_bad_jsonrpc_version():
    with pytest.raises(RpcError) as info:
        protocol.validate_request({"jsonrpc": "1.0", "method": "hello"})
    assert info.value.code == ErrorCode.INVALID_REQUEST


def test_validate_request_rejects_positional_params():
    with pytest.raises(RpcError) as info:
        protocol.validate_request({"jsonrpc": "2.0", "method": "hello", "params": [1]})
    assert info.value.code == ErrorCode.INVALID_PARAMS


def test_validate_request_rejects_bad_id():
    with pytest.raises(RpcError) as info:
        protocol.validate_request({"jsonrpc": "2.0", "method": "x", "id": {"a": 1}})
    assert info.value.code == ErrorCode.INVALID_REQUEST


def test_validate_request_notification_and_params_default():
    request = protocol.validate_request({"method": "ping"})
    assert isinstance(request, Request)
    assert request.is_notification is True
    assert request.params == {}
    assert request.id is None


def test_response_and_error_shapes():
    response = protocol.response("abc", {"ok": True})
    assert response == {"jsonrpc": "2.0", "id": "abc", "result": {"ok": True}}

    error = protocol.error_response("abc", ErrorCode.MISSING_DEPENDENCY, "no torch", {"missing": ["torch"]})
    assert error["jsonrpc"] == "2.0"
    assert error["id"] == "abc"
    assert error["error"]["code"] == int(ErrorCode.MISSING_DEPENDENCY)
    assert error["error"]["data"] == {"missing": ["torch"]}

    rpc_error = RpcError(ErrorCode.CONFIG_ERROR, "bad config")
    assert rpc_error.to_error() == {"code": int(ErrorCode.CONFIG_ERROR), "message": "bad config"}
    assert rpc_error.to_response(7)["id"] == 7


def test_serialize_is_one_compact_line_with_unicode():
    line = protocol.serialize(protocol.notification("train.progress", {"loss": 0.5, "note": "训练"}))
    assert "\n" not in line
    assert json.loads(line)["params"]["note"] == "训练"
    assert ": " not in line  # compact separators


def test_server_error_code_mapping():
    assert protocol.server_error_code("missing_dependency") == ErrorCode.MISSING_DEPENDENCY
    assert protocol.server_error_code("tool_not_found") == ErrorCode.TOOL_NOT_FOUND
    assert protocol.server_error_code("nope") == ErrorCode.INTERNAL_ERROR
