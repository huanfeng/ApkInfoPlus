; Windows 安装包（NSIS 3）。
; 平时由 scripts/package-windows.ps1 调用并传入：
;   APP_VERSION      版本号，如 0.4.0
;   APP_VERSION_NUM  四段版本号，如 0.4.0.0
;   ROOT             仓库根目录
;   SOURCE_EXE       已构建的 apkinfoplus.exe
;   OUT_FILE         输出路径
; 按当前用户安装（不需要管理员），安装完成即注册 .apk 等文件关联，
; 之后双击安装包即可用本程序打开；卸载时自动取消关联。

Unicode true
!include "MUI2.nsh"

!ifndef APP_VERSION
  !define APP_VERSION "0.4.0"
!endif
!ifndef APP_VERSION_NUM
  !define APP_VERSION_NUM "0.4.0.0"
!endif
!ifndef ROOT
  !define ROOT "..\.."
!endif
!ifndef SOURCE_EXE
  !define SOURCE_EXE "${ROOT}\target\release\apkinfoplus.exe"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "${ROOT}\dist\ApkInfoPlus-${APP_VERSION}-windows-x86_64-setup.exe"
!endif

!define APP_NAME "APK Info Plus"
!define APP_EXE "apkinfoplus.exe"
!define APP_PUBLISHER "huanfeng"
!define APP_URL "https://github.com/huanfeng/ApkInfoPlus"
!define APP_KEY "Software\ApkInfoPlus"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\ApkInfoPlus"

Name "${APP_NAME} ${APP_VERSION}"
BrandingText "${APP_NAME} ${APP_VERSION}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\ApkInfoPlus"
InstallDirRegKey HKCU "${APP_KEY}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

VIProductVersion "${APP_VERSION_NUM}"
VIAddVersionKey /LANG=2052 "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "FileDescription" "${APP_NAME} 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "MIT License"
VIAddVersionKey /LANG=2052 "CompanyName" "${APP_PUBLISHER}"

!define MUI_ICON "${ROOT}\crates\app\assets\windows\app.ico"
!define MUI_UNICON "${ROOT}\crates\app\assets\windows\app.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "$(RUN_APP)"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

LangString SEC_MAIN ${LANG_SIMPCHINESE} "APK Info Plus（必需）"
LangString SEC_MAIN ${LANG_ENGLISH} "APK Info Plus (required)"
LangString SEC_MAIN_DESC ${LANG_SIMPCHINESE} "程序文件、开始菜单快捷方式，并注册 .apk 等安装包的文件关联。"
LangString SEC_MAIN_DESC ${LANG_ENGLISH} "Program files, Start menu shortcut, and .apk (and related package) file associations."
LangString SEC_DESKTOP ${LANG_SIMPCHINESE} "创建桌面快捷方式"
LangString SEC_DESKTOP ${LANG_ENGLISH} "Create desktop shortcut"
LangString SEC_DESKTOP_DESC ${LANG_SIMPCHINESE} "在桌面放置一个 APK Info Plus 快捷方式。"
LangString SEC_DESKTOP_DESC ${LANG_ENGLISH} "Place an APK Info Plus shortcut on the desktop."
LangString RUN_APP ${LANG_SIMPCHINESE} "运行 APK Info Plus"
LangString RUN_APP ${LANG_ENGLISH} "Run APK Info Plus"
LangString REGISTERING ${LANG_SIMPCHINESE} "正在注册 .apk 等文件关联..."
LangString REGISTERING ${LANG_ENGLISH} "Registering file associations..."

Section "$(SEC_MAIN)" SecMain
  SectionIn RO
  SetShellVarContext current

  SetOutPath "$INSTDIR"
  File "${SOURCE_EXE}"
  File "${ROOT}\LICENSE"
  File "${ROOT}\README.md"
  ; 规则数据由 scripts/sync-libchecker-rules.py 生成，本地没同步时跳过。
  !if /FileExists "${ROOT}\crates\core\assets\libchecker\LICENSE"
    File "/oname=LICENSE-LibChecker-Rules.txt" "${ROOT}\crates\core\assets\libchecker\LICENSE"
  !endif

  CreateDirectory "$SMPROGRAMS\APK Info Plus"
  CreateShortCut "$SMPROGRAMS\APK Info Plus\APK Info Plus.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0

  ; 关联由程序自己写（与设置页「文件关联」同一套逻辑，按当前用户 HKCU）。
  DetailPrint "$(REGISTERING)"
  ExecWait '"$INSTDIR\${APP_EXE}" --register-association'

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "${APP_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "${APP_URL}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "$(SEC_DESKTOP)" SecDesktop
  CreateShortCut "$DESKTOP\APK Info Plus.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "$(SEC_MAIN_DESC)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "$(SEC_DESKTOP_DESC)"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  SetShellVarContext current
  ; 先让程序清掉自己写的 HKCU\Software\Classes 关联，再删文件。
  ExecWait '"$INSTDIR\${APP_EXE}" --unregister-association'

  Delete "$DESKTOP\APK Info Plus.lnk"
  Delete "$SMPROGRAMS\APK Info Plus\APK Info Plus.lnk"
  RMDir "$SMPROGRAMS\APK Info Plus"

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE-LibChecker-Rules.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "${APP_KEY}"
SectionEnd
