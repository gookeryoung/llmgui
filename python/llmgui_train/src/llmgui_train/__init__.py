"""llmgui training worker: dataset, estimation, export and stdio JSON-RPC.

Importing this package is intentionally cheap and dependency free.  Modules that
need torch/transformers/peft import them lazily inside their functions.
"""

from __future__ import annotations

__version__ = "0.1.0"
__all__ = ["PROTOCOL_VERSION", "__version__"]

#: Protocol version understood by the Rust CLI (``hello`` handshake).
PROTOCOL_VERSION = 1
