//! System tray icon for the standalone app.
//!
//! Closing the window hides it here instead of quitting, so the tunnel keeps
//! running while the app is out of the way.
//!
//! The tray deliberately holds no authority over app state. GTK widgets are not
//! `Send`, and the tray runs on the tokio runtime, so nothing here touches a
//! widget: the GTK thread pushes a [`TraySnapshot`] in, and menu activations go
//! back out as [`TrayCommand`]s for the GTK thread to carry out.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use ksni::menu::{MenuItem, StandardItem};
use ksni::{Status, ToolTip};

/// The connection, as coarsely as the tray needs to know it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TrayState {
    #[default]
    Disconnected,
    Connecting {
        profile: String,
    },
    Connected {
        profile: String,
        local_ip: Ipv4Addr,
        gateway: Ipv4Addr,
        full_tunnel: bool,
    },
}

/// Everything the tray renders. Pushed from the GTK thread on every change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraySnapshot {
    pub state: TrayState,
    pub window_visible: bool,
}

impl TraySnapshot {
    /// The state before anything has connected, with the window on screen.
    pub fn initial() -> Self {
        TraySnapshot {
            state: TrayState::Disconnected,
            window_visible: true,
        }
    }
}

/// A menu activation for the GTK thread to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ToggleWindow,
    Connect,
    Disconnect,
    Quit,
}

/// Commands queued by the tray, drained by the GTK main loop.
pub type CommandQueue = Arc<Mutex<Vec<TrayCommand>>>;

pub struct VpnTray {
    snapshot: TraySnapshot,
    commands: CommandQueue,
}

impl VpnTray {
    pub fn new(commands: CommandQueue) -> Self {
        VpnTray {
            snapshot: TraySnapshot::initial(),
            commands,
        }
    }

    /// Replace what the tray displays. Called from the tray's own task.
    pub fn set_snapshot(&mut self, snapshot: TraySnapshot) {
        self.snapshot = snapshot;
    }

    fn queue(&self, cmd: TrayCommand) {
        self.commands
            .lock()
            .expect("tray command queue lock poisoned")
            .push(cmd);
    }
}

impl ksni::Tray for VpnTray {
    fn id(&self) -> String {
        "draytek-vpn".to_string()
    }

    fn title(&self) -> String {
        "DrayTek SSL VPN".to_string()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Communications
    }

    fn status(&self) -> Status {
        Status::Active
    }

    /// Left click shows or hides the window — the usual close-to-tray gesture.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.queue(TrayCommand::ToggleWindow);
    }

    /// Empty: the icon is supplied as a pixmap by [`Self::icon_pixmap`].
    ///
    /// A themed name such as `network-vpn-symbolic` renders as one more
    /// monochrome network glyph and is indistinguishable from the system's own
    /// tray icons, so the VPN draws its own coloured shield instead.
    fn icon_name(&self) -> String {
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        match &self.snapshot.state {
            TrayState::Disconnected => draytek_vpn_trayicons::DISCONNECTED.clone(),
            TrayState::Connecting { .. } => draytek_vpn_trayicons::CONNECTING.clone(),
            TrayState::Connected { .. } => draytek_vpn_trayicons::CONNECTED.clone(),
        }
    }

    fn tool_tip(&self) -> ToolTip {
        let description = match &self.snapshot.state {
            TrayState::Disconnected => "Disconnected".to_string(),
            TrayState::Connecting { profile } => format!("Connecting: {profile}"),
            TrayState::Connected {
                profile,
                local_ip,
                gateway,
                full_tunnel,
            } => {
                let routing = if *full_tunnel {
                    "All traffic through tunnel"
                } else {
                    "Split tunnel"
                };
                format!("Connected: {profile}\nIP: {local_ip}\nGateway: {gateway}\n{routing}")
            }
        };

        ToolTip {
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
            title: "DrayTek SSL VPN".to_string(),
            description,
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        match &self.snapshot.state {
            TrayState::Disconnected => {
                items.push(label("Disconnected"));
                items.push(MenuItem::Separator);
                items.push(action("Connect", TrayCommand::Connect));
            }
            TrayState::Connecting { profile } => {
                items.push(label(&format!("Connecting: {profile}...")));
            }
            TrayState::Connected {
                profile,
                local_ip,
                gateway,
                full_tunnel,
            } => {
                items.push(label(&format!("Connected: {profile}")));
                items.push(label(&format!("IP: {local_ip}")));
                items.push(label(&format!("Gateway: {gateway}")));
                items.push(label(if *full_tunnel {
                    "Routing: all traffic"
                } else {
                    "Routing: split tunnel"
                }));
                items.push(MenuItem::Separator);
                items.push(action("Disconnect", TrayCommand::Disconnect));
            }
        }

        items.push(MenuItem::Separator);
        items.push(action(
            if self.snapshot.window_visible {
                "Hide Window"
            } else {
                "Show Window"
            },
            TrayCommand::ToggleWindow,
        ));
        items.push(MenuItem::Separator);
        items.push(action("Quit", TrayCommand::Quit));

        items
    }
}

/// Menu item that queues a command for the GTK thread.
fn action(text: &str, cmd: TrayCommand) -> MenuItem<VpnTray> {
    StandardItem {
        label: text.to_string(),
        enabled: true,
        visible: true,
        icon_name: String::new(),
        icon_data: Vec::new(),
        shortcut: Vec::new(),
        disposition: ksni::menu::Disposition::Normal,
        activate: Box::new(move |tray: &mut VpnTray| tray.queue(cmd)),
    }
    .into()
}

/// Non-interactive informational row.
fn label(text: &str) -> MenuItem<VpnTray> {
    StandardItem {
        label: text.to_string(),
        enabled: false,
        visible: true,
        icon_name: String::new(),
        icon_data: Vec::new(),
        shortcut: Vec::new(),
        disposition: ksni::menu::Disposition::Normal,
        activate: Box::new(|_| {}),
    }
    .into()
}
