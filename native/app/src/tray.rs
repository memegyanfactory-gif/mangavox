//! System tray icon: left-click brings MangaVox forward; the menu has Show and Quit.
use std::time::Duration;

use gpui_kit::*;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

pub fn install(main: AnyWindowHandle, cx: &mut App) {
    // Same artwork as the exe: icon resource 1 (see build.rs).
    let Ok(icon) = Icon::from_resource(1, Some((32, 32))) else { return };
    let (show, quit) = (MenuItem::new("Show MangaVox", true, None), MenuItem::new("Quit", true, None));
    let menu = Menu::new();
    if menu.append_items(&[&show, &quit]).is_err() {
        return;
    }
    let Ok(tray) = TrayIconBuilder::new().with_tooltip("MangaVox").with_icon(icon).with_menu(Box::new(menu)).with_menu_on_left_click(false).build() else {
        return;
    };
    let (show_id, quit_id) = (show.id().clone(), quit.id().clone());
    cx.spawn(async move |cx| {
        let _tray = tray;
        loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let mut bring = false;
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                    bring = true;
                }
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                if ev.id == quit_id {
                    cx.update(|cx| cx.quit());
                    return;
                }
                bring |= ev.id == show_id;
            }
            if bring {
                let _ = main.update(cx, |_, window, _| window.activate_window());
            }
        }
    })
    .detach();
}
