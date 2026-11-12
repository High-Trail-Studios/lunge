Lunge for macOS (interim, unsigned build)
=========================================

This is a single universal binary: it runs on Apple silicon and Intel Macs.
It is NOT signed with a Developer ID or notarized by Apple yet (a signed DMG
is planned). If your Mac is managed by an employer, check that running
unsigned tools is allowed before using it.

Requirements
------------
AWS CLI v2 on your PATH, with an SSO session set up (`aws configure sso`).

Run it
------
1. Open Terminal in the folder containing `lunge`.

2. macOS quarantines files downloaded through a browser and will refuse to
   open this one. Clear the quarantine flag on this file only:

       xattr -d com.apple.quarantine ./lunge

   (If it says "No such xattr", there's nothing to clear.)
   Alternatively: try to run it once, then System Settings -> Privacy &
   Security -> "Open Anyway".

3. Start it from Terminal:

       ./lunge

   Starting from Terminal matters for now: apps launched from Finder get a
   minimal PATH and may not find the `aws` command.

Optional: copy it somewhere on your PATH, e.g.

       sudo cp ./lunge /usr/local/bin/lunge

What it touches
---------------
Lunge only reads ~/.aws/config, profile names in ~/.aws/credentials, and the
AWS CLI's SSO token cache; it runs `aws sso login` when you ask it to. The
only file it writes is its display preferences (zoom, theme, window size) in
~/Library/Application Support/lunge/. Delete that folder and the binary to
remove it completely.
