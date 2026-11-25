; SerialLoom installer hooks (Tauri NSIS).
; The installer runs elevated, so SerialLoom's admin helper can install the
; bundled, Microsoft-signed virtual USB controller (usbip-win2) silently.
; Users never install anything separately.
;
; This file must stay UTF-8 with a byte order mark: without one, NSIS reads
; it in the system code page and garbles the translations.

; Writes a line to the install log in the installer's language, English for
; any other. Tauri includes this file before it loads the languages, so the
; text is picked when the line is written, by language id.
!macro SL_DETAIL EN DE ES PT RU ZH JA KO
  ${If} $LANGUAGE == 1031 ; German
    DetailPrint "${DE}"
  ${ElseIf} $LANGUAGE == 1034 ; Spanish
    DetailPrint "${ES}"
  ${ElseIf} $LANGUAGE == 1046 ; Portuguese (Brazil)
    DetailPrint "${PT}"
  ${ElseIf} $LANGUAGE == 1049 ; Russian
    DetailPrint "${RU}"
  ${ElseIf} $LANGUAGE == 2052 ; Simplified Chinese
    DetailPrint "${ZH}"
  ${ElseIf} $LANGUAGE == 1041 ; Japanese
    DetailPrint "${JA}"
  ${ElseIf} $LANGUAGE == 1042 ; Korean
    DetailPrint "${KO}"
  ${Else}
    DetailPrint "${EN}"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; An older SerialLoom Helper may be running from the files being replaced.
  ; "net stop" waits until it has stopped; it fails harmlessly if not installed.
  nsExec::Exec '"$SYSDIR\net.exe" stop SerialLoomHelper'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro SL_DETAIL \
    "Installing the virtual USB controller for SerialLoom's COM ports..." \
    "Virtueller USB-Controller für die COM-Ports von SerialLoom wird installiert..." \
    "Instalando el controlador USB virtual para los puertos COM de SerialLoom..." \
    "Instalando o controlador USB virtual para as portas COM do SerialLoom..." \
    "Установка виртуального USB-контроллера для COM-портов SerialLoom..." \
    "正在为 SerialLoom 的 COM 端口安装虚拟 USB 控制器..." \
    "SerialLoom の COM ポート用の仮想 USB コントローラーをインストールしています..." \
    "SerialLoom COM 포트용 가상 USB 컨트롤러 설치 중..."
  nsExec::ExecToLog '"$INSTDIR\serialloom.exe" --serialloom-admin "$TEMP\serialloom-reply-installer.json" install-usb'
  Pop $0
  ${If} $0 != 0
    !insertmacro SL_DETAIL \
      "Setup reported code $0. You can retry from SerialLoom > Settings." \
      "Die Einrichtung meldete Code $0. Sie können es unter SerialLoom > Einstellungen erneut versuchen." \
      "La instalación informó el código $0. Puede reintentarlo en SerialLoom > Configuración." \
      "A instalação informou o código $0. Você pode tentar de novo em SerialLoom > Configurações." \
      "Установка вернула код $0. Повторить можно в SerialLoom > Настройки." \
      "安装程序返回代码 $0。可在 SerialLoom > 设置 中重试。" \
      "セットアップがコード $0 を返しました。SerialLoom > 設定 から再試行できます。" \
      "설치 프로그램이 코드 $0을(를) 반환했습니다. SerialLoom > 설정에서 다시 시도할 수 있습니다."
  ${EndIf}

  ; Command-line tool for scripts and tests (built by beforeBuildCommand
  ; next to serialloom.exe).
  File "/oname=serialloom-cli.exe" "${MAINBINARYSRCPATH}\..\serialloom-cli.exe"

  ; SerialLoom Helper: creates, renumbers and removes SerialLoom's own ports
  ; without a UAC prompt each time. Can be removed in SerialLoom > Settings.
  !insertmacro SL_DETAIL \
    "Installing SerialLoom Helper..." \
    "SerialLoom Helper wird installiert..." \
    "Instalando SerialLoom Helper..." \
    "Instalando o SerialLoom Helper..." \
    "Установка SerialLoom Helper..." \
    "正在安装 SerialLoom Helper..." \
    "SerialLoom Helper をインストールしています..." \
    "SerialLoom Helper 설치 중..."
  nsExec::ExecToLog '"$INSTDIR\serialloom.exe" --serialloom-admin "$TEMP\serialloom-reply-installer-helper.json" install-service'
  Pop $0
  ${If} $0 != 0
    !insertmacro SL_DETAIL \
      "SerialLoom Helper was not installed (code $0). Windows will ask for permission for port changes; you can retry from SerialLoom > Settings." \
      "SerialLoom Helper wurde nicht installiert (Code $0). Windows fragt bei Port-Änderungen nach Berechtigung; Sie können es unter SerialLoom > Einstellungen erneut versuchen." \
      "SerialLoom Helper no se instaló (código $0). Windows pedirá permiso en cada cambio de puerto; puede reintentarlo en SerialLoom > Configuración." \
      "O SerialLoom Helper não foi instalado (código $0). O Windows pedirá permissão a cada alteração de porta; você pode tentar de novo em SerialLoom > Configurações." \
      "SerialLoom Helper не установлен (код $0). Windows будет запрашивать разрешение при изменении портов; повторить можно в SerialLoom > Настройки." \
      "未安装 SerialLoom Helper（代码 $0）。更改端口时 Windows 会请求权限；可在 SerialLoom > 设置 中重试。" \
      "SerialLoom Helper はインストールされませんでした（コード $0）。ポートを変更するたびに Windows が許可を求めます。SerialLoom > 設定 から再試行できます。" \
      "SerialLoom Helper가 설치되지 않았습니다(코드 $0). 포트를 변경할 때마다 Windows에서 권한을 요청합니다. SerialLoom > 설정에서 다시 시도할 수 있습니다."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Removes SerialLoom Helper, every virtual port and the drivers.
  !insertmacro SL_DETAIL \
    "Removing SerialLoom virtual ports and drivers..." \
    "Virtuelle Ports und Treiber von SerialLoom werden entfernt..." \
    "Quitando los puertos virtuales y controladores de SerialLoom..." \
    "Removendo as portas virtuais e os drivers do SerialLoom..." \
    "Удаление виртуальных портов и драйверов SerialLoom..." \
    "正在移除 SerialLoom 的虚拟串口和驱动..." \
    "SerialLoom の仮想ポートとドライバーを削除しています..." \
    "SerialLoom 가상 포트와 드라이버 제거 중..."
  nsExec::ExecToLog '"$INSTDIR\serialloom.exe" --serialloom-admin "$TEMP\serialloom-reply-uninstaller.json" uninstall-all'
  Pop $0
  Delete "$INSTDIR\serialloom-cli.exe"
!macroend
