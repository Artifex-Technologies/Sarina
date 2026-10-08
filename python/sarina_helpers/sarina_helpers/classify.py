"""Semantic classifier for ``desktop-audit``: nearest category prototype to a description.

Returns only the expensive, threshold-independent part, (label, score). The Go
side applies its similarity threshold and its content-hash cache itself.
"""

from __future__ import annotations

import hashlib
from typing import Any

from .embeddings import rank_by_similarity

#: One descriptive sentence per category bucket we are willing to emit. Prototype
#: vectors, not a trained classifier: adding a bucket costs one line.
CATEGORY_PROTOTYPES: tuple[tuple[tuple[str, ...], str], ...] = (
    (('Network', 'Security'),
     'network scanning, port scanning, packet capture, sniffing, wireless and '
     'wifi auditing, spoofing, proxies and remote network attacks'),
    (('System', 'Security'),
     'password cracking, hash cracking, digital forensics, malware and rootkit '
     'analysis, privilege escalation and host exploitation'),
    (('Development', 'Security'),
     'disassembly, decompilation, debugging, fuzzing and reverse engineering of '
     'binaries and executables'),
    (('Utility', 'Security'),
     'encryption, decryption, cryptography, hashing, encoding and steganography'),
    (('Development',),
     'software development, programming languages, compilers, build systems, '
     'version control and source code editing'),
    (('Network',),
     'web browsing, downloading, file transfer, email, chat and general '
     'internet client software'),
    (('Graphics',),
     'image editing, photo manipulation, drawing, vector graphics and viewing '
     'pictures'),
    (('AudioVideo',),
     'playing, recording and editing audio, music and video'),
    (('Office',),
     'word processing, spreadsheets, presentations, documents and note taking'),
    (('Science',),
     'scientific computing, mathematics, statistics, data analysis and plotting'),
    (('Settings',),
     'configuring the desktop, appearance, input devices and system preferences'),
    (('System',),
     'system administration, managing processes, services, disks, filesystems '
     'and monitoring the machine'),
    (('Utility',),
     'small general purpose desktop utilities, archiving, clipboard and text tools'),
)


def prototypes_fingerprint() -> str:
    """sha256 of the prototype set; the Go side refuses a helper whose set differs from its own."""
    serialized = '\n'.join(f'{";".join(c)}|{s}' for c, s in CATEGORY_PROTOTYPES)
    return hashlib.sha256(serialized.encode('utf-8')).hexdigest()


class Classifier:
    """Nearest prototype for a description. Embeds the prototypes once."""

    def __init__(self, model: Any):
        self._model = model
        self._labels = [';'.join(c) for c, _ in CATEGORY_PROTOTYPES]
        vectors = model.embed([s for _, s in CATEGORY_PROTOTYPES])
        self._prototypes = list(zip(self._labels, vectors))

    def classify(self, text: str) -> tuple[str | None, float]:
        """Return (label, score); label is ';'-joined categories, None if no signal."""
        if not text.strip():
            return None, 0.0
        ranked = rank_by_similarity(self._model.embed_one(text), self._prototypes, top_k=1)
        return ranked[0] if ranked else (None, 0.0)
