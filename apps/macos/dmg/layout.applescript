-- The Finder view settings that make up the install window, applied to a mounted scratch
-- image so make-assets.sh can capture the .DS_Store Finder writes.
--
-- Every number here is paired with background.html: the window is exactly the background's
-- 680x440, and the two icon positions are the centres of the cards drawn in the art. Change
-- one and you must change the other, or the icons drift off their surfaces and the labels
-- lose the contrast the whole design is built to guarantee.
on run argv
    set volName to item 1 of argv
    set appName to item 2 of argv
    tell application "Finder"
        tell disk volName
            open
            set current view of container window to icon view
            set toolbar visible of container window to false
            set statusbar visible of container window to false
            -- the path bar follows a GLOBAL Finder preference, so on a Mac that has it
            -- switched on it silently takes ~28pt off the bottom of the icon view and
            -- clips the background with it. Turning it off per-window is stored in the
            -- .DS_Store, so the disk image opens the same either way.
            set pathbar visible of container window to false
            -- Finder's window bounds INCLUDE the title bar, so a 440-tall rect leaves the
            -- icon view 32pt short and clips the bottom of the background. 440 + 32 gives
            -- the art its full height. The exact title bar height moves between macOS
            -- releases, which is the other reason the foot of the background is a soft
            -- gradient rather than an edge: a few pixels either way cannot look wrong.
            set the bounds of container window to {200, 120, 880, 592}

            set opts to the icon view options of container window
            set arrangement of opts to not arranged
            set icon size of opts to 128
            set text size of opts to 12
            set label position of opts to bottom
            set shows item info of opts to false
            set background picture of opts to file ".background:background.tiff"

            -- Finder keys the position on the item's NAME, so this has to be the name the
            -- bundle actually ships under -- "ADI Dev.app" for a dev build. Position it by
            -- the wrong name and Finder silently leaves the icon wherever it lands.
            set position of item appName of container window to {176, 210}
            set position of item "Applications" of container window to {504, 210}

            update without registering applications
            delay 1
            close
        end tell
    end tell
end run
