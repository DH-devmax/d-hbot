Option Explicit

Dim shell, fso, baseDir, dhPath, wslPath
Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

baseDir = fso.GetParentFolderName(WScript.ScriptFullName)
dhPath = fso.BuildPath(baseDir, "DH-BOT.exe")
wslPath = shell.ExpandEnvironmentStrings("%WSL_EXE%")

If wslPath <> "%WSL_EXE%" And fso.FileExists(wslPath) Then
    shell.Run Chr(34) & wslPath & Chr(34) & " --remote-debugging-port=9222 --remote-allow-origins=http://127.0.0.1:9222", 0, False
    WScript.Sleep 3000
End If

shell.Run Chr(34) & dhPath & Chr(34), 1, False
