# Approved real header fixtures

The project owner approved committing these header bytes from the read-only BD Demo Reel sample. Each is cut after the last complete header KLV and before padding/essence; these are deliberately not playable MXFs.

| Fixture | Source filename | Bytes |
| --- | --- | ---: |
| audio.header | 150928A01.64CD450AF9124D40A.mxf | 157378 |
| video.header (DNxHR SQ) | V01.64CD446D_F8BBCF8BBC00DV.mxf | 127530 |
| rgba.header | V02.64CD447F_F8C79F8C793D6V.mxf | 130162 |

`msmFMID.pmr` is the sample's complete 344-entry PMR inventory. Header fixtures retain the original partition's reserved header extent, even though its trailing filler has been omitted.
