use tauri::{Emitter, WebviewWindow};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND, ICoreWebView2_11, ICoreWebView2_2,
    ICoreWebView2Environment9,
};
use webview2_com::{
    ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler, ExecuteScriptCompletedHandler,
};
use windows_core::{HSTRING, Interface, PCWSTR};

pub fn install(window: &WebviewWindow) {
    let target = window.clone();
    if let Err(error) = window.with_webview(move |platform| {
        // Tauri runs this callback on the WebView2 UI thread.
        let result = unsafe {
            platform.controller().CoreWebView2().and_then(|webview| {
                let environment: ICoreWebView2Environment9 =
                    webview.cast::<ICoreWebView2_2>()?.Environment()?.cast()?;
                webview.cast::<ICoreWebView2_11>()?.add_ContextMenuRequested(
                    &ContextMenuRequestedEventHandler::create(Box::new(move |sender, args| {
                        let (Some(sender), Some(args)) = (sender, args) else {
                            return Ok(());
                        };
                        let mut point = Default::default();
                        args.Location(&mut point)?;
                        let script = HSTRING::from(format!(
                            r#"(() => {{
                                const card = document.elementFromPoint(
                                    {} / devicePixelRatio, {} / devicePixelRatio
                                )?.closest("[data-mirror-hub]");
                                const label = card?.closest("[data-mirror-label]")?.dataset.mirrorLabel;
                                return card?.dataset.mirrorHub && label
                                    ? [card.dataset.mirrorHub, label] : null;
                            }})()"#,
                            point.x, point.y
                        ));
                        let deferral = args.GetDeferral()?;
                        let done = deferral.clone();
                        let target = target.clone();
                        let environment = environment.clone();
                        let handler = ExecuteScriptCompletedHandler::create(Box::new(move |status, json| {
                            let result = status.and_then(|()| {
                                let Ok((hub, label)) = serde_json::from_str::<(String, String)>(&json) else {
                                    return Ok(());
                                };
                                let label = HSTRING::from(label);
                                let item = environment.CreateContextMenuItem(
                                    PCWSTR(label.as_ptr()),
                                    None,
                                    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
                                )?;
                                item.add_CustomItemSelected(
                                    &CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                                        let _ = target.emit("topology-flip-horizontal", &hub);
                                        Ok(())
                                    })),
                                    &mut 0,
                                )?;
                                let items = args.MenuItems()?;
                                let mut count = 0;
                                items.Count(&mut count)?;
                                items.InsertValueAtIndex(count, &item)
                            });
                            let completion = done.Complete();
                            result.and(completion)
                        }));
                        if let Err(error) = sender.ExecuteScript(PCWSTR(script.as_ptr()), &handler) {
                            let _ = deferral.Complete();
                            return Err(error);
                        }
                        Ok(())
                    })),
                    &mut 0,
                )
            })
        };
        if let Err(error) = result {
            tracing::warn!(%error, "could not register the topology context menu");
        }
    }) {
        tracing::warn!(%error, "could not access the main webview");
    }
}
