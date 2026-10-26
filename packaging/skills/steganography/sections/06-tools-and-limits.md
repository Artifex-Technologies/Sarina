# Helper programs and the tool's limits

stego_inspect is read-only, works without any outside program, and names the ones an analyst would add:
- exiftool: every metadata tag of almost any format (run by the tool when installed).
- binwalk: file signatures and carving inside firmware and images (run when installed).
- zsteg: PNG and BMP LSB in every channel order and bit plane, plus zlib-compressed payloads; the thorough version of the built-in
  extraction.
- steghide and stegseek: steghide hides in JPEG and WAV with a password; stegseek tries a wordlist against it.
- outguess, stegdetect: JPEG coefficient methods.
- foremost, scalpel: carve embedded files out.
- strings, xxd, file: the basics.
None is run unless the report says so; the report lists which are installed. On Labyrinth, exiftool and binwalk are the useful two to
add (Agora or pacman) when the owner does this often.

Limits the tool states itself: files over 64 MB and images over 40 megapixels are not read; JPEG coefficients, keyed LSB, spectrograms,
video frames and document internals are not analysed, only pointed at; a statistic is a sign, an extraction is proof; nothing is
written anywhere but a temporary folder for extracted bytes.

Sarina changes nothing: she does not strip metadata, re-save images or carve files herself; she says what to run (exiftool -all=,
tail -c, unzip) and what it would do.
