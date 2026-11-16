; SerialLoom installer hooks (Tauri NSIS).
; The installer runs elevated, so SerialLoom's admin helper can install the
; bundled, Microsoft-signed virtual USB controller (usbip-win2) silently.
; Users never install anything separately.

!macro NSIS_HOOK_PREINSTALL
  ; An older SerialLoom Helper may be running from the files being replaced.
  ; "net stop" waits until it has stopped; it fails harmlessly if not installed.
  nsExec::Exec '"$SYSDIR\net.exe" stop SerialLoomHelper'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Installing the virtual USB controller for SerialLoom's COM ports..."
  nsExec::ExecToLog '"$INSTDIR\serialloom.exe" --serialloom-admin "$TEMP\serialloom-reply-installer.json" install-usb'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Setup reported code $0. You can retry from SerialLoom > Settings."
  ${EndIf}

  ; Command-line tool for scripts and tests (built by beforeBuildCommand
  ; next to serialloom.exe).
  File "/oname=serialloom-cli.exe" "${MAINBINARYSRCPATH}\..\serialloom-cli.exe"

  ; SerialLoom Helper: creates, renumbers and removes SerialLoom's own ports
  ; without a UAC prompt each time. Can be removed in SerialLoom > Settings.
  DetailPrint "Installing SerialLoom Helper..."
  nsExec::ExecToLog '"$INSTDIR\serialloom.exe" --serialloom-admin "$TEMP\serialloom-reply-installer-helper.json" install-service'
  Pop $0
  ${If} $0 != 0
    DetailPrint "SerialLoom Helper was not installed (code $0). Windows will ask for permission for port changes; you can retry from SerialLoom > Settings."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Removes SerialLoom Helper, every virtual port and the drivers.
  DetailPrint "Removing SerialLoom virtual ports and drivers..."
  nsExec::ExecToLog '"$INSTDIR\serialloom.exe" --serialloom-admin "$TEMP\serialloom-reply-uninstaller.json" uninstall-all'
  Pop $0
  Delete "$INSTDIR\serialloom-cli.exe"
!macroend
