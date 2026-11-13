use super::{State, view::*};
use rune::{ClipboardContent, Color, El, col, row};
use serde_json::json;

pub(super) fn view(s: &State, mut body: El) -> El {
    let saved = s.preferences.value.mcp.enabled;
    let enabled = s
        .preferences
        .value
        .mcp
        .effective(s.preferences.mcp_override);
    body = body
        .child(title(super::locale::tr("MCP · AI agents"), 22.0))
        .child(
            label(
                super::locale::tr(
                    "Connect an MCP-compatible agent to your WhatsApp account through Brazap.",
                ),
                12.0,
                MUTED,
            )
            .wrap(),
        )
        .child(super::preferences::toggle(
            s,
            "Enable MCP server",
            "Let local AI agents read and send WhatsApp messages through Brazap. Off by default.",
            saved,
            |p| p.mcp.enabled = !p.mcp.enabled,
        ));
    if s.preferences
        .mcp_override
        .is_some_and(|value| value != saved)
    {
        body = body.child(label(super::locale::tr("The BRAZAP_MCP environment variable overrides this setting until Brazap restarts without it."), 11.0, MUTED).wrap());
    }
    if !enabled {
        return body.child(label(super::locale::tr("The MCP server is off. Agents cannot connect and no tools are offered until you turn it on."), 12.0, MUTED).wrap());
    }
    let executable = std::env::current_exe().unwrap_or_else(|_| "brazap".into());
    let config = serde_json::to_string_pretty(&json!({
        "mcpServers": { "brazap": {
            "command": executable,
            "args": ["--data-dir", s.config.data_dir.to_string_lossy(), "--mcp"]
        }}
    }))
    .unwrap_or_default();
    let copy = config.clone();
    body = body
        .child(col().w_full().padding(18.0).gap(12.0).radius(14.0).fill(Color::WHITE.alpha(0.025)).border(1.0, BORDER)
            .child(title(super::locale::tr("Connect your agent"), 15.0))
            .child(label(super::locale::tr("1. Keep Brazap open and connected to WhatsApp.\n2. Add this configuration to your agent’s MCP settings.\n3. Restart or reconnect the agent’s MCP client."), 12.0, MUTED).wrap())
            .child(label(config, 11.0, INK).wrap().w_full().padding(12.0).radius(10.0).fill(Color::BLACK.alpha(0.2)))
            .child(button("Copy MCP configuration", move |_, tx| {
                tx.copy_rich(ClipboardContent::new(copy.clone()));
            })))
        .child(label(super::locale::tr("Uses a local stdio connection and the same session and SQLite archive as the app. No separate WhatsApp pairing is needed. Agents with access can send real messages and save attachments; configure approvals in your agent."), 12.0, MUTED).wrap())
        .child(title(super::locale::tr("Available tools"), 17.0));
    if let Some(tools) = crate::mcp::tools().as_array() {
        for tool in tools {
            let name = tool["name"].as_str().unwrap_or_default();
            let description = tool["description"].as_str().unwrap_or_default();
            let read_only = tool["annotations"]["readOnlyHint"]
                .as_bool()
                .unwrap_or(false);
            body = body.child(
                col()
                    .w_full()
                    .padding(14.0)
                    .gap(8.0)
                    .radius(12.0)
                    .border(1.0, BORDER)
                    .child(
                        row()
                            .w_full()
                            .center_y()
                            .gap(12.0)
                            .child(title(name, 13.0).grow(1.0))
                            .child(label(
                                super::locale::tr(
                                    &(if read_only { "Read only" } else { "Action" }),
                                ),
                                10.0,
                                MINT,
                            )),
                    )
                    .child(label(super::locale::tr(description), 12.0, MUTED).wrap()),
            );
        }
    }
    body
}
