; Установщик Rustspace для Windows (Inno Setup 6).
; Ставит в профиль пользователя (без прав администратора), поэтому клиент
; потом обновляется сам: скачивает этот же установщик и запускает тихо.
;   iscc /DVersion=0.3.0 /DSource=..\..\target\release packaging\windows\rustspace.iss

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Source
  #define Source "..\..\target\release"
#endif

[Setup]
AppId={{6D3B2F7E-8A51-4C1E-9F0B-52A7C4E1D9A3}
AppName=Rustspace
AppVersion={#Version}
AppPublisher=Quddy
AppPublisherURL=https://github.com/QuddyMusic/Rustspace
AppSupportURL=https://github.com/QuddyMusic/Rustspace/issues
DefaultDirName={localappdata}\Programs\Rustspace
DefaultGroupName=Rustspace
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
OutputDir=..\..\dist
OutputBaseFilename=rustspace-setup-x86_64
SetupIconFile=rustspace.ico
UninstallDisplayIcon={app}\rustspace.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Windows 10 и 11.
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=force
RestartApplications=no
LicenseFile=..\..\LICENSE

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\rustspace.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\NOTICE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Rustspace"; Filename: "{app}\rustspace.exe"
Name: "{autodesktop}\Rustspace"; Filename: "{app}\rustspace.exe"; Tasks: desktopicon

[Run]
; И после обычной, и после тихой установки (обновление из клиента) — запуск.
Filename: "{app}\rustspace.exe"; Description: "{cm:LaunchProgram,Rustspace}"; Flags: nowait postinstall
