# Text and documents

Invisible characters: zero-width space (U+200B), zero-width non-joiner (U+200C), zero-width joiner (U+200D), word joiner (U+2060),
the byte-order mark (U+FEFF), Unicode tag characters (U+E0000-E007F) and direction marks are invisible in most editors. Two kinds used
as 0 and 1 carry bits between visible letters; tag characters carry ASCII directly (each is a letter plus 0xE0000). stego_inspect with
action text counts them by kind and decodes the usual schemes. These same characters smuggle instructions into text given to AI
assistants; Sarina's input guard refuses text with them, and a file full of them is a reason for suspicion either way.

Whitespace: trailing spaces and tabs at line ends (the snow method), double spaces, tab-vs-space choices; invisible in a viewer,
visible to a tool that counts them.

Homoglyphs: letters from another script that look the same (Cyrillic а, е, о, р, с; Greek ο), used to hide a mark or to make a
misleading address. Mixed scripts in one word are the sign.

Linguistic steganography: the message in the first letters of lines or words, in word choice, in capitalisation; needs a reader who
knows the rule, and no tool finds it in general. Look at first letters when a text reads oddly.

Documents: a docx, xlsx, pptx or odt is a zip; data hides as an extra entry, in a comment, in hidden text (white on white, font size
1, a hidden paragraph style), in custom properties. A PDF is a tree of objects: unused objects, attachments, JavaScript, white text,
layers. stego_inspect finds embedded file signatures and strings; unzip -l on a docx and a PDF object listing (pdfdetach, qpdf) are the
next tools.
