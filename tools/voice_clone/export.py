"""Thin launcher for `python -m piper.train.export_onnx`.

`VitsModel.load_from_checkpoint` also calls `torch.load(..., weights_only=True)`
internally, hitting the exact same PosixPath problem `train.py` works around
-- see that file's docstring for the full explanation. Kept as a separate,
tiny launcher rather than folding into train.py since export is a distinct
step with its own CLI (`--checkpoint`/`--output-file`).

Usage: identical to `python -m piper.train.export_onnx`, e.g.
    python export.py --checkpoint logs/.../last.ckpt --output-file voice.onnx
"""

from __future__ import annotations

import pathlib

import torch.serialization

torch.serialization.add_safe_globals([
    (pathlib.PurePosixPath, 'pathlib.PosixPath'),
    pathlib.PurePosixPath,
])

from piper.train.export_onnx import main  # noqa: E402

if __name__ == '__main__':
    main()
