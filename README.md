# Music

An Ice Commander plugin that plays audio files, with the rest of the folder as a
playlist.

It registers one viewer, `audio-view`, and nothing else. The window it describes
carries a `media` node: the host's player does the playing and draws the
transport. The plugin decides which track plays and what else is in the list.

`ic_plugin_init` refuses the console host (`IC_ERR_NOT_THIS_HOST`).

## Files

`.mp3 .flac .ogg .oga .opus .wav .m4a .aac .wma .aiff .mp4a`, by extension only,
case-insensitive.

## The window

- On open the plugin lists the folder through `fs_list`, keeps the files with
  one of the extensions above, sorts them case-insensitively and marks the
  opened one. If the folder cannot be listed or the file is not in it, the
  playlist is that one file.
- Top: the cover, the headline, the album, the player, back and next buttons
  (`Left` / `Right`) and the position `n / total`.
- Below: every track as a row; the current one is marked ▶ and cannot be
  pressed.
- Back, next or a row: the plugin reads the new track's tag, sets it to
  autoplay and answers `{"redescribe":true}`. At either end, or on the current
  row, nothing happens.
- The opened track starts playing at once.

## Tags and cover

- Headline: `artist — title` when the tag has both, the title alone when it has
  only that, otherwise the file name. The album goes under it.
- Only an ID3v2 tag at the start of the file is read: the 10-byte header, then
  exactly the length it declares. `tags.rs` parses ID3v2.3/2.4 `TIT2`, `TPE1`
  and `TALB` in Latin-1, UTF-16 (with or without BOM) and UTF-8, and the first
  `APIC` that parses as the cover.
- The host asks for the cover as `part:cover`. A track without one gets
  `assets/music.svg`.

## Known limitations

- When a track ends, playback stops. The host reports the end (`ended`),
  but the plugin does not act on it yet, so it does not move on to the next track.
- On a non-local filesystem (a server, an archive) the GTK and web hosts copy
  the one file into a scratch folder and open the viewer there, so the playlist
  is that single track.
- Only an ID3v2.3/2.4 tag at the head of the file is read; ID3v2.2, ID3v1,
  appended ID3v2, Vorbis comments, MP4 and ASF tags are not. Such tracks show
  the file name and the placeholder cover.
- An ID3v2 extended header hides the whole tag. Unsynchronisation is not
  reversed, so a cover or UTF-16 text in such a tag comes out corrupted.
- `.opus`, `.wma` and `.aiff` are claimed here, but the GTK host's player
  cannot decode them.

## Building

```sh
./build.sh          # release build, libraries collected into bin/
./test.sh           # cargo test --workspace
./deploy-local.sh   # copies bin/ libraries into the plugin folder (IC_PLUGIN_DIR overrides)
```

Then switch it on in **Settings → Plugins** and restart.

## Licence

MIT or Apache-2.0, at your option. The icon in `src/audio-view/assets/` is
covered by [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md). Contributions
are taken under the [DCO](DCO); sign off with `git commit -s`.
