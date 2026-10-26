# Method: "is something hidden in this file?"

1. Run stego_inspect on the file (or read its result, if it was looked at for you). Read it top to bottom: type vs extension; structure;
   metadata; data after the end; signatures inside; strings; the LSB measurement and extraction; which helpers were and were not run.
2. Findings that settle it: a payload extracted (quoted text, or a file signature in the LSB stream); data after the end marker; an
   embedded file; a hidden text decoded from invisible characters; a metadata comment with the message. Report these as found, with the
   offsets and the text.
3. Signs that do not settle it: an equalised LSB statistic without a readable extraction (noise, or a keyed or encrypted payload);
   unusual chunks; high-entropy trailing bytes (an encrypted payload, or garbage). Report them as signs and say what would settle them:
   the original file to diff against, the tool and password that were used, or a specific extraction (action lsb with another order or
   more bits).
4. Nothing found: say what was checked and what was not (JPEG coefficients, keyed hiding, spectrograms, other bit orders), so "no sign"
   is not read as "nothing there".
5. Next steps you can offer: diff against an original; action lsb with order r/g/b/rgba and bits 2-4; carve the trailing data to a
   file and inspect that; for text, action text; for a docx, list the zip entries. Name an outside tool (zsteg, steghide, stegseek,
   binwalk, exiftool) only as something the owner could install; never as something you ran unless the report says it ran.

Reading the LSB report: "ones about 50%" is normal for photos and for payloads alike; the pairs-of-values p-value near 1 means
equalised pairs; the extraction line is the one that decides. A payload with a 32-bit length header is the plain scheme most tutorials
use; text until zeros is the other.

Puzzle habits: the payload is often another puzzle (base64, a cipher): hand it to the crypto tools. A flag looks like flag{...}. The
extension lies more often than not.
