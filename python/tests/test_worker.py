"""Tests for :mod:`llmgui_train.worker` (in-process and subprocess)."""

from __future__ import annotations

import io
import json
import threading

import pytest

from conftest import has_full_training_stack, notifications, response_by_id
from llmgui_train import worker


def frame(method, params=None, request_id="1"):
    payload = {"jsonrpc": "2.0", "method": method}
    if request_id is not None:
        payload["id"] = request_id
    if params is not None:
        payload["params"] = params
    return json.dumps(payload)


def parse_lines(stdout: str):
    return [json.loads(line) for line in stdout.splitlines() if line.strip()]


def make_worker(script: str, runner=None, **kwargs):
    stdout = io.StringIO()
    stderr = io.StringIO()
    kwargs.setdefault("enforce_dependencies", False)
    instance = worker.Worker(
        stdin=io.StringIO(script),
        stdout=stdout,
        stderr=stderr,
        train_runner=runner,
        **kwargs,
    )
    code = instance.serve()
    return code, parse_lines(stdout.getvalue()), stderr.getvalue()


# ---------------------------------------------------------------------------
# subprocess tests: the real worker entry point
# ---------------------------------------------------------------------------


def test_subprocess_hello(worker_runner):
    process = worker_runner([frame("hello", {"protocol": 1}, "hello-1")])
    assert process.returncode == 0, process.stderr
    frames = parse_lines(process.stdout)
    hello = response_by_id(frames, "hello-1")
    result = hello["result"]
    assert result["event"] == "hello_ok"
    assert result["protocol"] == 1
    assert result["server"] == "llmgui_train"
    assert "capabilities" in result
    assert result["training"]["ready"] is False or isinstance(result["training"]["ready"], bool)


def test_subprocess_estimate(worker_runner):
    process = worker_runner([frame("estimate", {"model": "7b", "method": "qlora"}, "est-1")])
    assert process.returncode == 0, process.stderr
    result = response_by_id(parse_lines(process.stdout), "est-1")["result"]
    assert result["settings"]["method"] == "qlora"
    assert result["total_bytes"] < 24 * 1024**3


def test_subprocess_unknown_method(worker_runner):
    process = worker_runner([frame("does.not.exist", {}, "u-1")])
    frames = parse_lines(process.stdout)
    error = response_by_id(frames, "u-1")["error"]
    assert error["code"] == -32601
    assert "available" in error["data"] or "supported" in error["data"]


def test_subprocess_error_recovery(worker_runner):
    script = "\n".join(
        [
            "{not json",
            json.dumps([1, 2, 3]),
            frame("ping", {}, "p-1"),
        ]
    )
    process = worker_runner(script.splitlines())
    frames = parse_lines(process.stdout)
    codes = [frame["error"]["code"] for frame in frames if "error" in frame]
    assert -32700 in codes
    assert -32600 in codes
    assert response_by_id(frames, "p-1")["result"]["pong"] is True


def test_subprocess_params_must_be_object(worker_runner):
    payload = json.dumps(
        {"jsonrpc": "2.0", "id": "x-1", "method": "hello", "params": [1, 2]}
    )
    process = worker_runner([payload])
    frames = parse_lines(process.stdout)
    codes = [frame["error"]["code"] for frame in frames if "error" in frame]
    assert -32602 in codes


def test_subprocess_hello_protocol_mismatch(worker_runner):
    process = worker_runner([frame("hello", {"protocol": 99}, "hello-99")])
    error = response_by_id(parse_lines(process.stdout), "hello-99")["error"]
    assert error["code"] == -32018
    assert error["data"]["protocol"] == 1


@pytest.mark.skipif(
    has_full_training_stack(),
    reason="full training stack installed; missing-dependency path not applicable",
)
def test_subprocess_train_start_missing_deps(worker_runner, config_file):
    process = worker_runner(
        [frame("train.start", {"config_path": str(config_file)}, "train-1")],
    )
    assert process.returncode == 1, process.stderr
    error = response_by_id(parse_lines(process.stdout), "train-1")["error"]
    assert error["code"] == -32010
    assert error["data"]["code"] == "missing_dependency"
    assert error["data"]["missing"]


def test_subprocess_train_start_config_error(worker_runner, tmp_path):
    process = worker_runner(
        [frame("train.start", {"config_path": str(tmp_path / "missing.json")}, "train-2")],
    )
    assert process.returncode == 1, process.stderr
    error = response_by_id(parse_lines(process.stdout), "train-2")["error"]
    assert error["code"] == -32011


def test_subprocess_train_start_bad_params(worker_runner):
    process = worker_runner([frame("train.start", {}, "train-3")])
    assert process.returncode == 1, process.stderr
    error = response_by_id(parse_lines(process.stdout), "train-3")["error"]
    assert error["code"] == -32602


def test_subprocess_export_gguf_tool_not_found(worker_runner, tmp_path):
    model_dir = tmp_path / "model"
    model_dir.mkdir()
    process = worker_runner(
        [
            frame(
                "export.gguf",
                {"model_dir": str(model_dir), "output_path": str(tmp_path / "m.gguf")},
                "gguf-1",
            )
        ]
    )
    error = response_by_id(parse_lines(process.stdout), "gguf-1")["error"]
    assert error["code"] == -32015
    assert error["data"]["code"] == "tool_not_found"


def test_subprocess_dry_run_train(worker_runner, config_file):
    process = worker_runner(
        [frame("train.start", {"config_path": str(config_file)}, "train-4")],
        args=["--dry-run"],
    )
    assert process.returncode == 0, process.stderr
    frames = parse_lines(process.stdout)
    started = response_by_id(frames, "train-4")["result"]
    assert started["started"] is True
    done = notifications(frames, "done")
    assert done and done[0]["params"]["ok"] is True
    assert done[0]["params"]["result"]["stub"] is True


# ---------------------------------------------------------------------------
# in-process tests: worker dispatch, notifications, cancellation
# ---------------------------------------------------------------------------


def test_inprocess_notification_gets_no_response():
    code, frames, _ = make_worker(frame("ping", {}, request_id=None))
    assert code == 0
    assert frames == []


def test_inprocess_train_success_streams_done(config_file):
    captured = {}

    def runner(config, on_event=None, cancel_event=None):
        captured["config"] = config
        on_event({"kind": "train.progress", "step": 1, "total_steps": 1, "loss": 0.5, "lr": 1e-4})
        return {"ok": True, "adapter_dir": config.output_dir, "steps": 1, "metrics": {"loss": 0.5}}

    script = "\n".join(
        [
            frame("train.start", {"config_path": str(config_file)}, "t-1"),
            frame("ping", {}, "t-2"),
        ]
    )
    code, frames, stderr = make_worker(script, runner=runner)
    assert code == 0, stderr
    assert response_by_id(frames, "t-1")["result"]["started"] is True
    progress = notifications(frames, "train.progress")
    assert progress and progress[0]["params"]["step"] == 1
    done = notifications(frames, "done")
    assert done and done[0]["params"]["ok"] is True
    assert done[0]["params"]["result"]["steps"] == 1
    assert captured["config"].base_model == "Qwen2.5-7B-Instruct"


def test_inprocess_train_cancel(config_file):
    started = threading.Event()

    def runner(config, on_event=None, cancel_event=None):
        on_event({"kind": "train.progress", "step": 1})
        started.set()
        cancelled = cancel_event.wait(timeout=5)
        return {"ok": True, "cancelled": cancelled, "steps": 1, "metrics": {}}

    script = "\n".join(
        [
            frame("train.start", {"config_path": str(config_file)}, "c-1"),
            frame("train.cancel", {}, "c-2"),
        ]
    )
    code, frames, stderr = make_worker(script, runner=runner)
    assert code == 0, stderr
    assert started.wait(timeout=5)
    cancel = response_by_id(frames, "c-2")["result"]
    assert cancel == {"cancelled": True, "running": True}
    done = notifications(frames, "done")
    assert done and done[0]["params"]["result"]["cancelled"] is True


def test_inprocess_cancel_without_training():
    code, frames, _ = make_worker(frame("train.cancel", {}, "c-3"))
    assert code == 0
    assert response_by_id(frames, "c-3")["result"] == {"cancelled": False, "running": False}


def test_inprocess_runner_missing_dependency(config_file):
    from llmgui_train.trainer import MissingDependency

    def runner(config, on_event=None, cancel_event=None):
        raise MissingDependency(["torch"])

    code, frames, _ = make_worker(
        frame("train.start", {"config_path": str(config_file)}, "m-1"), runner=runner
    )
    assert code == 0
    assert any(
        frame.get("method") == "error" and frame["params"]["code"] == "missing_dependency"
        for frame in frames
    )


def test_inprocess_runner_failure(config_file):
    def runner(config, on_event=None, cancel_event=None):
        raise RuntimeError("boom")

    code, frames, stderr = make_worker(
        frame("train.start", {"config_path": str(config_file)}, "f-1"), runner=runner
    )
    assert code == 0
    assert any(f.get("method") == "error" and f["params"]["code"] == "training_failed" for f in frames)
    assert "boom" in stderr


def test_inprocess_inline_config(dataset_file):
    captured = {}

    def runner(config, on_event=None, cancel_event=None):
        captured["config"] = config
        return {"ok": True, "steps": 0, "metrics": {}}

    inline = {
        "base_model": "model-7b",
        "dataset": str(dataset_file),
        "method": "lora",
        "seq_len": 128,
    }
    code, frames, _ = make_worker(
        frame("train.start", {"config": inline}, "i-1"), runner=runner
    )
    assert code == 0
    assert response_by_id(frames, "i-1")["result"]["started"] is True
    assert captured["config"].method == "lora"


def test_worker_handles_broken_stdout_gracefully():
    class BrokenStream(io.StringIO):
        def write(self, _data):  # pragma: no cover - defensive path
            raise OSError("stdout gone")

    instance = worker.Worker(
        stdin=io.StringIO(frame("hello", {"protocol": 1})),
        stdout=BrokenStream(),
        stderr=io.StringIO(),
    )
    with pytest.raises(OSError):
        instance.serve()
