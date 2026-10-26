; SerialLoom installer hooks (Tauri NSIS).
; The installer runs elevated, so SerialLoom's admin helper can install the
; bundled, Microsoft-signed virtual USB controller (usbip-win2) silently.
; Users never install anything separately.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Installing the virtual USB controller for SerialLoom's COM ports..."
  nsExec::ExecToLog '"$INSTDIR\serialloom.exe" --serialloom-admin "$TEMP\serialloom-reply-installer.json" install-usb'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Setup reported code $0. You can retry from SerialLoom > Settings."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removing SerialLoom virtual ports and drivers..."
  nsExec::ExecToLog '"$INSTDIR\serialloom.exe" --serialloom-admin "$TEMP\serialloom-reply-uninstaller.json" uninstall-all'
  Pop $0
!macroend
