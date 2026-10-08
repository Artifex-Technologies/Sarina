"""Thin launcher for `python -m piper.train`, working around a real
compatibility gap rather than a security hole to route past: Lightning's
CLI does an internal `torch.load(ckpt_path, weights_only=True)` pre-parse at
startup (unconditionally -- the CLI's own `--weights_only` flag only affects
the later `trainer.fit()` call, not this one), and these older Piper
training checkpoints embed a plain `pathlib.PosixPath` in their
hyperparameters (from being saved on Linux). Two separate problems, both
handled below: PyTorch 2.6+'s weights_only default won't unpickle it at all
without being told to trust it, and even once trusted, a *real* PosixPath
can't be instantiated on Windows (a platform limitation, not a security
check).

Fix: `torch.serialization.add_safe_globals` accepts a `(callable, "name")`
tuple that registers an arbitrary callable under an explicit key -- so
`PurePosixPath` (a pure, OS-agnostic string container, buildable on any OS)
is registered under the exact key the checkpoint references
("pathlib.PosixPath"), without touching the real `pathlib.PosixPath` class.
Safe for checkpoints from a trusted source (e.g. rhasspy/piper-checkpoints
on Hugging Face): we never read the stored path's value, only need to get
past unpickling it, and nothing else's weights_only checking is affected.

Usage: identical to `python -m piper.train`, e.g.
    python train.py fit --data.csv_path ... --ckpt_path ...
"""

from __future__ import annotations

import pathlib

import torch.serialization

torch.serialization.add_safe_globals([
    (pathlib.PurePosixPath, 'pathlib.PosixPath'),
    # clean_checkpoint.py re-saves a cleaned checkpoint whose surviving
    # `dataset` field is a real PurePosixPath (round-tripped through the
    # remap above) -- allow it directly too. Unlike PosixPath, PurePosixPath
    # does no OS check, so this is safe on every platform.
    pathlib.PurePosixPath,
])

from piper.train.__main__ import main  # noqa: E402

if __name__ == '__main__':
    main()
