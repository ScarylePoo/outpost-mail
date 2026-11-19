; Extra installer steps for Windows (see docs/release.md).
;
; These entries let Windows offer Outpost as a mail program: once they are
; written, Outpost appears under Settings > Apps > Default apps, where the
; user can choose it for email links (MAILTO). They do not make Outpost the
; default; since Windows 10 only the user can do that, in Settings.
;
; Everything is written under the current user (the installer is per-user
; and needs no administrator), and removed again on uninstall.

!define OUTPOST_PROGID "OutpostMail.Url.mailto"
!define OUTPOST_CLIENT "Software\Clients\Mail\Outpost Mail"

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\${OUTPOST_PROGID}" "" "URL:MailTo Protocol"
  WriteRegStr SHCTX "Software\Classes\${OUTPOST_PROGID}" "URL Protocol" ""
  WriteRegStr SHCTX "Software\Classes\${OUTPOST_PROGID}\DefaultIcon" "" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr SHCTX "Software\Classes\${OUTPOST_PROGID}\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  WriteRegStr SHCTX "${OUTPOST_CLIENT}" "" "Outpost Mail"
  WriteRegStr SHCTX "${OUTPOST_CLIENT}\Capabilities" "ApplicationName" "Outpost Mail"
  WriteRegStr SHCTX "${OUTPOST_CLIENT}\Capabilities" "ApplicationDescription" "A desktop mail program."
  WriteRegStr SHCTX "${OUTPOST_CLIENT}\Capabilities\URLAssociations" "mailto" "${OUTPOST_PROGID}"
  WriteRegStr SHCTX "Software\RegisteredApplications" "Outpost Mail" "${OUTPOST_CLIENT}\Capabilities"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue SHCTX "Software\RegisteredApplications" "Outpost Mail"
  DeleteRegKey SHCTX "${OUTPOST_CLIENT}"
  DeleteRegKey SHCTX "Software\Classes\${OUTPOST_PROGID}"
!macroend
