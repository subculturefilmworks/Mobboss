# Mob Boss

Media management for Avid Media Composer: scan, identify, and sort Avid MXF media by project, without needing Avid running.

Mob Boss reads Avid media folders (`Avid MediaFiles/MXF/*`, `OMFI MediaFiles`) on local and shared storage. It identifies each file from its own MXF header, using the Avid database files (`msmMMOB.mdb`, `msmFMID.pmr`) only as a fast path. It can then move, copy, or collect media per project.

Inspired by [MDV/MDVx](https://github.com/DJFio/MDV) by DJFio.

**Status:** early development.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Avid and Media Composer are trademarks of Avid Technology, Inc. Mob Boss is not affiliated with or endorsed by Avid.
