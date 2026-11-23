Lunge for Windows (interim, unsigned build)
===========================================

This is a portable build for 64-bit Intel/AMD Windows 10 and 11. Nothing is
installed: lunge.exe runs from wherever you unzip it. It is NOT code-signed
yet. If your PC is managed by an employer, check that running unsigned tools
is allowed before using it.

Requirements
------------
AWS CLI v2 (the Windows installer puts `aws` on your PATH). To add an SSO
session from Lunge you need AWS CLI 2.34.22 or newer (check with
`aws --version`; rerun the AWS CLI installer to update). Older versions can
still add one in a terminal with `aws configure sso`.

Run it
------
1. Before unzipping, right-click the zip -> Properties -> tick "Unblock" ->
   OK. (Or in PowerShell: Unblock-File .\lunge-<version>-windows-x86_64.zip)
   This clears the "downloaded from the internet" mark so Windows doesn't
   warn about each file.

2. Unzip it anywhere you like, e.g. a Tools folder in your user folder.

3. Double-click lunge.exe. If Windows shows "Windows protected your PC",
   choose "More info" -> "Run anyway". That warning appears because the
   build isn't signed.

   If Smart App Control is turned on (Windows 11), Windows blocks unsigned
   apps with no way to run them anyway. Lunge can't run there until it is
   signed.

Optional: right-click lunge.exe -> "Pin to Start" or "Pin to taskbar".

What it touches
---------------
Lunge only reads %USERPROFILE%\.aws\config, profile names in
%USERPROFILE%\.aws\credentials, and the AWS CLI's SSO token cache; it runs
the AWS CLI when you ask it to. It changes your AWS config only when you
ask: adding a session or profile goes through the AWS CLI, and renaming a
profile changes only that profile's header line, after a preview, with a
backup saved next to the file. The only other file it writes is its display
preferences (zoom, theme, window size) in %APPDATA%\Lunge\. To remove it
completely, delete that folder and the folder you unzipped.
