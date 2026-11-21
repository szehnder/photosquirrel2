# Manual check: Open in player (ADR-0134)

The tests check that Open in player asks the platform to open the right path, through a recording opener. They
cannot check that the platform's command really starts the default player. This check does, on each platform,
with a clip made for it. It takes about two minutes per platform and touches nothing but a temporary folder.

## Prerequisites

- The release build under test (`photosquirrel` and `photosquirrel-cli`) and `ffmpeg` on `PATH`.
- A default application for `.mov` files (on Linux: `xdg-mime query default video/quicktime` names one).

## Steps

1. A scratch library with one clip (on Windows, use the same commands in PowerShell with `$env:TEMP`):

   ```sh
   d=$(mktemp -d); mkdir "$d/clips"
   ffmpeg -nostdin -loglevel error -f lavfi -i testsrc2=duration=12:size=1920x1080:rate=24 -c:v mpeg4 "$d/clips/clip.mov"
   photosquirrel-cli --data-dir "$d/data" sources add "$d/clips"
   photosquirrel-cli --data-dir "$d/data" scan
   photosquirrel --data-dir "$d/data"
   ```

2. In the Library, double-click the clip. Expect a sharp frame (not a blurry thumbnail), a play mark with `0:12` in
   its bottom-left corner, no "Large preview unavailable" notice, and **Open in player** at the top right.
3. Press **O**. Expect the default player to open the clip and PhotoSquirrel to stay responsive.
4. Close the player. In the inspector's **All copies**, click the play button on the copy. Expect the player again.
5. Quit PhotoSquirrel, rename `$d/clips` to `$d/clips-away`, start it again and select the clip. Expect the copy's
   play button to be dimmed with the tooltip "Source offline", and **O** in the loupe to show "Could not open the
   video" with the reason.
6. Check that the clip is unchanged (the same size and modification time as in step 1), then remove `$d`.

## Results

| Platform | Date | Build | Player | Result |
|---|---|---|---|---|
| Linux (Manjaro, KDE Plasma) | | 0.5.1 | VLC (`xdg-open`) | |
| macOS | | 0.5.1 | QuickTime Player (`open`) | |
| Windows 11 | | 0.5.1 | Media Player (`explorer.exe`) | |
