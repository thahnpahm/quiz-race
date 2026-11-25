; Bộ cài Windows cho Quiz Race (Inno Setup 6).
; Bấm đúp là cài: không có trang hỏi nào, cài cho tài khoản đang dùng (không cần admin),
; tạo lối tắt Start menu + Desktop, cài xong tự mở game. Gỡ trong Settings → Apps.
; Build: iscc /DAppVersion=2.0.0 installer\QuizRace.iss   (cần build exe trước: cargo build --release)

#ifndef AppVersion
  #define AppVersion "dev"
#endif
#ifndef ExePath
  #define ExePath "..\target\release\quiz-race.exe"
#endif

[Setup]
; AppId cố định để các bản sau cài đè lên bản cũ
AppId={{5B0E7C2A-3D41-4F7B-9E6C-6A1D2F8B9C34}
AppName=Quiz Race
AppVersion={#AppVersion}
AppPublisher=thahnpahm
DefaultDirName={autopf}\QuizRace
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
OutputDir=..\out
OutputBaseFilename=QuizRace-{#AppVersion}-win-x64-setup
SetupIconFile=..\icon.ico
UninstallDisplayIcon={app}\QuizRace.exe
UninstallDisplayName=Quiz Race
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
DisableWelcomePage=yes
DisableReadyPage=yes
DisableFinishedPage=yes
CloseApplications=force

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; DestName: "QuizRace.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Quiz Race"; Filename: "{app}\QuizRace.exe"
Name: "{autodesktop}\Quiz Race"; Filename: "{app}\QuizRace.exe"

[Run]
Filename: "{app}\QuizRace.exe"; Flags: nowait skipifsilent
