use super::ChatWidget;
use crate::bottom_pane::ColumnWidthMode;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::key_hint;
use crate::render::renderable::ColumnRenderable;
use codex_core::plugins::ConfiguredMarketplacePluginSummary;
use codex_core::plugins::ConfiguredMarketplaceSummary;
use codex_core::plugins::MarketplacePluginSourceSummary;
use codex_core::plugins::PluginsManager;
use codex_utils_absolute_path::AbsolutePathBuf;
use crossterm::event::KeyCode;
use ratatui::style::Stylize;
use ratatui::text::Line;

const PLUGINS_SELECTION_VIEW_ID: &str = "plugins-selection";

impl ChatWidget {
    pub(crate) fn add_plugins_output(&mut self) {
        if !self.plugins_enabled() {
            self.add_info_message("Plugins are not enabled.".to_string(), None);
            return;
        }

        let additional_roots = AbsolutePathBuf::try_from(self.config.cwd.clone())
            .ok()
            .into_iter()
            .collect::<Vec<_>>();
        match PluginsManager::new(self.config.codex_home.clone())
            .list_marketplaces_for_config(&self.config, &additional_roots)
        {
            Ok(marketplaces) => {
                if marketplaces.is_empty() {
                    self.add_info_message("No plugins available.".to_string(), None);
                } else {
                    self.open_plugins_popup(&marketplaces);
                }
            }
            Err(err) => self.add_error_message(format!("Failed to load plugins: {err}")),
        }
    }

    fn open_plugins_popup(&mut self, marketplaces: &[ConfiguredMarketplaceSummary]) {
        self.bottom_pane
            .show_selection_view(self.plugins_popup_params(marketplaces));
    }

    fn plugins_popup_params(
        &self,
        marketplaces: &[ConfiguredMarketplaceSummary],
    ) -> SelectionViewParams {
        let total_plugins = marketplaces
            .iter()
            .map(|marketplace| marketplace.plugins.len())
            .sum();
        let installed_plugins = marketplaces
            .iter()
            .flat_map(|marketplace| marketplace.plugins.iter())
            .filter(|plugin| plugin.installed)
            .count();
        let enabled_plugins = marketplaces
            .iter()
            .flat_map(|marketplace| marketplace.plugins.iter())
            .filter(|plugin| plugin.enabled)
            .count();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Plugins".bold()));
        header.push(Line::from(
            "Browse plugins discovered from configured marketplaces.".dim(),
        ));
        header.push(Line::from(
            format!(
                "Enabled {enabled_plugins}, installed {installed_plugins}, total {total_plugins} across {} marketplaces.",
                marketplaces.len()
            )
            .dim(),
        ));

        let mut items = Vec::with_capacity(total_plugins);
        for marketplace in marketplaces {
            for plugin in &marketplace.plugins {
                let display_name = plugin_display_name(plugin);
                items.push(SelectionItem {
                    name: display_name.clone(),
                    name_prefix_spans: vec![format!("[{}] ", marketplace.name).dim()],
                    description: Some(plugin_description(plugin)),
                    selected_description: Some(plugin_selected_description(marketplace, plugin)),
                    search_value: Some(plugin_search_value(marketplace, plugin, &display_name)),
                    ..Default::default()
                });
            }
        }

        SelectionViewParams {
            view_id: Some(PLUGINS_SELECTION_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(plugins_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Type to search plugins".to_string()),
            col_width_mode: ColumnWidthMode::AutoAllRows,
            ..Default::default()
        }
    }
}

fn plugins_popup_hint_line() -> Line<'static> {
    Line::from(vec![
        "Press ".into(),
        key_hint::plain(KeyCode::Esc).into(),
        " to close. Browse only.".into(),
    ])
}

fn plugin_display_name(plugin: &ConfiguredMarketplacePluginSummary) -> String {
    plugin
        .interface
        .as_ref()
        .and_then(|interface| interface.display_name.clone())
        .unwrap_or_else(|| plugin.name.clone())
}

fn plugin_status_label(plugin: &ConfiguredMarketplacePluginSummary) -> &'static str {
    if plugin.enabled {
        "Installed (enabled)"
    } else if plugin.installed {
        "Installed"
    } else {
        "Available"
    }
}

fn plugin_description(plugin: &ConfiguredMarketplacePluginSummary) -> String {
    let status = plugin_status_label(plugin);
    let details = plugin
        .interface
        .as_ref()
        .and_then(|interface| interface.short_description.clone())
        .or_else(|| {
            plugin
                .interface
                .as_ref()
                .and_then(|interface| interface.category.clone())
        });
    match details {
        Some(details) => format!("{status} - {details}"),
        None => status.to_string(),
    }
}

fn plugin_selected_description(
    marketplace: &ConfiguredMarketplaceSummary,
    plugin: &ConfiguredMarketplacePluginSummary,
) -> String {
    let mut parts = vec![
        plugin_status_label(plugin).to_string(),
        format!("Marketplace: {}", marketplace.name),
    ];

    if let Some(interface) = &plugin.interface {
        if let Some(developer_name) = &interface.developer_name {
            parts.push(format!("Developer: {developer_name}"));
        }
        if let Some(category) = &interface.category {
            parts.push(format!("Category: {category}"));
        }
        if !interface.capabilities.is_empty() {
            parts.push(format!(
                "Capabilities: {}",
                interface.capabilities.join(", ")
            ));
        }
        if let Some(description) = interface
            .long_description
            .clone()
            .or_else(|| interface.short_description.clone())
        {
            parts.push(description);
        }
    }

    let path = match &plugin.source {
        MarketplacePluginSourceSummary::Local { path } => path,
    };
    parts.push(format!("Source: {}", path.display()));

    parts.join(". ")
}

fn plugin_search_value(
    marketplace: &ConfiguredMarketplaceSummary,
    plugin: &ConfiguredMarketplacePluginSummary,
    display_name: &str,
) -> String {
    let mut parts = vec![
        display_name.to_string(),
        plugin.name.clone(),
        plugin.id.clone(),
        marketplace.name.clone(),
    ];
    if let Some(interface) = &plugin.interface {
        if let Some(short_description) = &interface.short_description {
            parts.push(short_description.clone());
        }
        if let Some(developer_name) = &interface.developer_name {
            parts.push(developer_name.clone());
        }
        if let Some(category) = &interface.category {
            parts.push(category.clone());
        }
        if !interface.capabilities.is_empty() {
            parts.push(interface.capabilities.join(" "));
        }
    }
    parts.join(" ")
}
