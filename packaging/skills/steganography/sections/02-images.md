# Images

Formats matter. PNG, BMP and GIF store pixels (PNG compressed losslessly), so a pixel-LSB payload survives saving. JPEG stores DCT
coefficients and re-quantises on every save: pixel-LSB hiding does not survive a JPEG round trip, so JPEG steganography works on the
coefficients (steghide, outguess, F5, jsteg) and needs the same tool, usually with a password, to extract. A PNG converted from a JPEG
carries JPEG's blocky statistics but no coefficient payload.

What stego_inspect does for an image:
1. Type by magic bytes against the extension; a mismatch (a zip named .png) is itself a finding.
2. Structure: PNG chunks (IHDR size and colour type, text chunks, unusual or private chunks), JPEG segments (APP0 JFIF, APP1 Exif/XMP,
   COM comments, unexpected APPn), GIF comment extensions.
3. Metadata text: EXIF make/model/software/artist/comment, GPS presence, XMP descriptions, PNG tEXt/zTXt/iTXt. Metadata can also tell
   which program last saved the file.
4. Data after the end marker (IEND, FFD9, GIF trailer) and file signatures inside: an appended or embedded file.
5. Strings worth reading: URLs, flag{...}, long base64, key-like words.
6. LSB measurement per channel: the share of ones (about half for noise or a payload; far from half for flat art) and the
   pairs-of-values chi-square test (sequential LSB replacement makes the counts of values 2k and 2k+1 equal; p near 1 is a sign). The
   sign is weak on noisy or synthetic images and misses keyed or partial payloads.
7. LSB extraction in the common orders (rgb interleaved, one channel, rgba, bgr; most and least significant bit first; a 32-bit length
   header or text until zeros): a stream that starts like text or a known file is a payload, extracted and quoted. The verdict rests on
   this, not on the statistic.
8. diff of two images (the original and the suspect): changes only in the lowest bit are the signature of LSB hiding.

Palette images (indexed PNG, GIF): the payload can sit in the palette indices, or in the palette order; the tool measures the indices.
Alpha channel: a constant alpha hides nothing; a varying one can carry bits. Higher bit planes (bit 1, 2) are also used; action lsb
with bits 2-4 reads them.

Not covered in code, and said so: keyed LSB (needs the password), JPEG coefficient methods (need their tool), hiding in the image's
look itself (a drawing with a message), and images whose payload is spread by a secret permutation.
