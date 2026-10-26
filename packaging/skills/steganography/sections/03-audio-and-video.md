# Audio and video

WAV (PCM): each sample is a number; the lowest bit of each is the classic hiding place, read in sample order. stego_inspect parses the
RIFF chunks (fmt, data, LIST/INFO text such as title, artist, comment, and chunks a player would skip), measures the sample LSBs and
extracts them in both bit orders. 16-bit music has noisy LSBs; a payload shows when the extracted stream reads as text or a file.

Compressed audio (MP3, Ogg, FLAC, AAC): a sample-LSB payload does not survive lossy compression, so hiding there uses the format's own
structure: ID3 tags and comments (text metadata), padding bytes between MP3 frames, unused bits in frame headers, or data appended
after the last frame. The tool reads the metadata and trailing data and reports the rest as not checked.

Spectrogram steganography: an image or text drawn into the frequencies of a sound, visible in a spectrogram viewer (Audacity, sox
spectrogram) and audible as a chirp. The tool does not render spectrograms; when a file sounds like noise or a sweep, suggest looking at
its spectrogram.

Video: a container (MP4, MKV, AVI) with streams; data hides in extra streams, metadata atoms, between frames, or in frames' LSBs. The
tool identifies the container, reads trailing data and strings, and says that frame-level analysis needs ffmpeg to extract frames
first.

Phone recordings and voice notes carry metadata (recording app, device, sometimes location) worth knowing about before sharing.
