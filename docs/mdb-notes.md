# Avid MDB investigation

The supplied `msmMMOB.mdb` is 3,199,888 bytes. Its opening bytes are `_USER`, followed by property names such as `_VERSION`, `_SAVED_AAF_AUDIO_RATE_NUM`, `_SAVED_AAF_AUDIO_RATE_DEN`, and `_SAVED_AAF_AUDIO_LENGTH`. This is not SQLite, Microsoft Access, or an MXF stream.

The final 24 bytes are:

```
a4 43 4d a5 48 64 72 d7 00 00 49 49 01 00 00 00
80 37 07 00 f8 9b 29 00
```

The eight-byte Bento container signature is present. `49 49` indicates little-endian storage. Interpreting the final two 32-bit words as little-endian gives a table-of-contents offset of 472,960 and length 2,726,904; their sum is 3,199,864, exactly the file length minus the 24-byte label. This strongly supports absolute TOC offsets for this sample. The trailer also contains flags/version/block-size fields whose semantics are not established here.

Strings and FourCC-like tokens (`CLSD`, `TKPA`, `TKDA`) support the OMF/Bento interpretation. Finding strings alone cannot establish relationships between filenames, source packages, projects, and tracks. A correct reader needs the container object/property/type tables and Avid's object graph. A reliable MDB inventory parser is deferred rather than guessing associations from neighboring strings.

Scanning recognizes MDB presence and reports the validated Bento TOC as information, and uses validated PMR entries, the SQLite cache, and MXF headers. An MDB-only directory still triggers auto's quick policy, but every file lacking an indexed header is header-scanned. Missing database entries can be reported from PMR or a previous index; an initial MDB-only scan cannot report entries that exist only in MDB.

Next steps: independently verify the label layout, absolute offsets and TOC encoding against an OMF/Bento reference implementation, then resolve object properties and compare package IDs against MXF/PMR for the complete sample. No MDB bytes are changed.
