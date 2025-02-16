# gitosu

This project is created for a very specific usecase: tracking changes in osu! maps using git.
The map itself is just a text file, so it is pretty easily handled by git.

# Usage

## Watcher mode (default)

Run `gitosu` in your osu!(stable/lazer) exports directory and it will start watching for new .osz files.

Now you can click **Export** in editor and gitosu will automatically create a git repository for the map in your exports folder, or, if it already exists, commit all the changes.

Exports and repositories directories can be overriden using CLI arguments (`-e, --exports` and `-r, --repositories`)

## Manual import

Run `gitosu import <your .osz file>` in your repositories folder (or use `-r`) to manually create a new commit for the map.

You can use `--use-repository <NAME>` to manually set target repository's name.

## Naming

Repository's name is automatically determined from the file name and ignores duplicate numbers (the ` (XXX)` at the end of the file.
