# What steganography is

Cryptography hides what a message says; steganography hides that a message exists. The hidden data rides inside a carrier (an image,
a sound file, a document, plain text) that still looks and works like an ordinary file. Often both are used: the payload is encrypted,
then hidden.

Where data hides, from the plainest to the subtlest:
- Appended after the end of the format: a zip stuck on the end of a PNG or JPEG. Viewers stop at the end marker, archive tools find the
  zip. The simplest trick, and the first to check.
- In metadata: EXIF comments, PNG text chunks, XMP, ID3 tags, a GIF comment, a WAV INFO list. Visible to any metadata reader.
- In unusual structure: a private chunk in a PNG, an extra APP segment in a JPEG, a chunk a WAV player skips.
- In the least significant bits of samples: each pixel's colour value or audio sample has its lowest bit replaced by a payload bit;
  the eye and ear cannot tell. Sequential LSB (from the first pixel on) is what simple tools and tutorials do; keyed LSB spreads the bits
  by a password; JPEG tools (steghide, outguess, F5) change DCT coefficients instead of pixels.
- In text: zero-width and tag characters between visible letters, trailing spaces and tabs, homoglyphs (a Cyrillic а for a Latin a),
  word choice and capitalisation patterns.
- In containers: a docx or pdf is itself a zip or a tree of objects; extra entries or unused objects hide files.

What the tool does: stego_inspect looks for all of these in code, says what it found, what it measured, and what it could not check.
Steganalysis finds signs; it proves a payload only when it extracts one. Absence of signs is not proof of absence: a keyed or
encrypted payload in a noisy photo is close to undetectable without the key.
