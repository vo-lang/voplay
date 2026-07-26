use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::EngineId;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PluginId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PluginContributionKind {
    ComponentSchema,
    ComponentStore,
    System,
    Resource,
    Event,
    AssetPipeline,
    RenderFeature,
    Material,
    Diagnostics,
    EditorInspector,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginContribution {
    pub kind: PluginContributionKind,
    pub stable_id: u64,
    pub version: u32,
    pub name: String,
    pub required_capability: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginDescriptor {
    pub id: PluginId,
    pub version: u32,
    pub name: String,
    pub dependencies: Vec<PluginId>,
    pub required_capabilities: BTreeSet<u64>,
    pub contributions: Vec<PluginContribution>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginRegistryConfig {
    pub max_plugins: usize,
    pub max_dependencies_per_plugin: usize,
    pub max_contributions_per_plugin: usize,
    pub max_total_bytes: usize,
}

impl Default for PluginRegistryConfig {
    fn default() -> Self {
        Self {
            max_plugins: 1024,
            max_dependencies_per_plugin: 128,
            max_contributions_per_plugin: 4096,
            max_total_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginRegistryError {
    Closed,
    InvalidConfig,
    WrongEngine,
    RegistryFrozen,
    RegistryNotFrozen,
    PluginCapacity,
    DependencyCapacity,
    ContributionCapacity,
    ByteCapacity,
    InvalidPlugin,
    InvalidContribution,
    DuplicatePlugin,
    DuplicatePluginName,
    DuplicateContribution,
    UnknownDependency,
    DependencyCycle,
    MissingCapability,
}

pub struct PluginRegistry {
    engine: EngineId,
    config: PluginRegistryConfig,
    capabilities: BTreeSet<u64>,
    plugins: BTreeMap<PluginId, PluginDescriptor>,
    ordered: Vec<PluginId>,
    total_bytes: usize,
    frozen: bool,
    fingerprint: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginRegistryOwnerSnapshot {
    pub closed: bool,
    pub plugins: usize,
    pub ordered_plugins: usize,
    pub contributions: usize,
    pub total_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginRegistryShutdownReport {
    pub released_plugins: usize,
    pub released_contributions: usize,
    pub released_bytes: usize,
}

impl PluginRegistry {
    pub fn new(
        engine: EngineId,
        config: PluginRegistryConfig,
        capabilities: BTreeSet<u64>,
    ) -> Result<Self, PluginRegistryError> {
        if !engine.is_valid() {
            return Err(PluginRegistryError::WrongEngine);
        }
        if config.max_plugins == 0
            || config.max_dependencies_per_plugin == 0
            || config.max_contributions_per_plugin == 0
            || config.max_total_bytes == 0
            || capabilities.contains(&0)
        {
            return Err(PluginRegistryError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            capabilities,
            plugins: BTreeMap::new(),
            ordered: Vec::new(),
            total_bytes: 0,
            frozen: false,
            fingerprint: 0,
            closed: false,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub const fn is_frozen(&self) -> bool {
        self.frozen
    }

    pub fn owner_snapshot(&self) -> PluginRegistryOwnerSnapshot {
        PluginRegistryOwnerSnapshot {
            closed: self.closed,
            plugins: self.plugins.len(),
            ordered_plugins: self.ordered.len(),
            contributions: self
                .plugins
                .values()
                .map(|plugin| plugin.contributions.len())
                .sum(),
            total_bytes: self.total_bytes,
        }
    }

    pub fn shutdown(&mut self) -> PluginRegistryShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.plugins.clear();
        self.ordered.clear();
        self.total_bytes = 0;
        self.frozen = false;
        self.fingerprint = 0;
        PluginRegistryShutdownReport {
            released_plugins: snapshot.plugins,
            released_contributions: snapshot.contributions,
            released_bytes: snapshot.total_bytes,
        }
    }

    pub fn register(&mut self, mut plugin: PluginDescriptor) -> Result<(), PluginRegistryError> {
        self.ensure_open()?;
        if self.frozen {
            return Err(PluginRegistryError::RegistryFrozen);
        }
        validate_plugin(&plugin, self.config)?;
        if self.plugins.len() == self.config.max_plugins {
            return Err(PluginRegistryError::PluginCapacity);
        }
        plugin.dependencies.sort();
        plugin.dependencies.dedup();
        plugin
            .contributions
            .sort_by(|left, right| (left.kind, left.stable_id).cmp(&(right.kind, right.stable_id)));
        if self.plugins.contains_key(&plugin.id) {
            return Err(PluginRegistryError::DuplicatePlugin);
        }
        if self
            .plugins
            .values()
            .any(|registered| registered.name == plugin.name)
        {
            return Err(PluginRegistryError::DuplicatePluginName);
        }
        let bytes = descriptor_bytes(&plugin)
            .and_then(|bytes| self.total_bytes.checked_add(bytes))
            .filter(|bytes| *bytes <= self.config.max_total_bytes)
            .ok_or(PluginRegistryError::ByteCapacity)?;
        self.total_bytes = bytes;
        self.plugins.insert(plugin.id, plugin);
        Ok(())
    }

    pub fn freeze(&mut self) -> Result<u64, PluginRegistryError> {
        self.ensure_open()?;
        if self.frozen {
            return Ok(self.fingerprint);
        }
        let mut contribution_keys = BTreeSet::new();
        for plugin in self.plugins.values() {
            if !plugin
                .required_capabilities
                .iter()
                .all(|capability| self.capabilities.contains(capability))
                || plugin.contributions.iter().any(|contribution| {
                    contribution
                        .required_capability
                        .is_some_and(|capability| !self.capabilities.contains(&capability))
                })
            {
                return Err(PluginRegistryError::MissingCapability);
            }
            for dependency in &plugin.dependencies {
                if !self.plugins.contains_key(dependency) {
                    return Err(PluginRegistryError::UnknownDependency);
                }
            }
            for contribution in &plugin.contributions {
                if !contribution_keys.insert((contribution.kind, contribution.stable_id)) {
                    return Err(PluginRegistryError::DuplicateContribution);
                }
            }
        }
        self.ordered = topological_order(&self.plugins)?;
        self.fingerprint = registry_fingerprint(&self.ordered, &self.plugins);
        self.frozen = true;
        Ok(self.fingerprint)
    }

    fn ensure_open(&self) -> Result<(), PluginRegistryError> {
        if self.closed {
            Err(PluginRegistryError::Closed)
        } else {
            Ok(())
        }
    }

    pub fn fingerprint(&self) -> Result<u64, PluginRegistryError> {
        self.frozen
            .then_some(self.fingerprint)
            .ok_or(PluginRegistryError::RegistryNotFrozen)
    }

    pub fn ordered_plugins(
        &self,
    ) -> Result<impl ExactSizeIterator<Item = &PluginDescriptor>, PluginRegistryError> {
        if !self.frozen {
            return Err(PluginRegistryError::RegistryNotFrozen);
        }
        Ok(self.ordered.iter().map(|id| &self.plugins[id]))
    }

    pub fn contribution(
        &self,
        kind: PluginContributionKind,
        stable_id: u64,
    ) -> Result<Option<(&PluginDescriptor, &PluginContribution)>, PluginRegistryError> {
        if !self.frozen {
            return Err(PluginRegistryError::RegistryNotFrozen);
        }
        Ok(self.ordered.iter().find_map(|id| {
            let plugin = &self.plugins[id];
            plugin
                .contributions
                .iter()
                .find(|contribution| {
                    contribution.kind == kind && contribution.stable_id == stable_id
                })
                .map(|contribution| (plugin, contribution))
        }))
    }
}

fn validate_plugin(
    plugin: &PluginDescriptor,
    config: PluginRegistryConfig,
) -> Result<(), PluginRegistryError> {
    if plugin.id.0 == 0
        || plugin.version == 0
        || !valid_name(&plugin.name)
        || plugin.dependencies.contains(&plugin.id)
        || plugin.required_capabilities.contains(&0)
    {
        return Err(PluginRegistryError::InvalidPlugin);
    }
    if plugin.dependencies.len() > config.max_dependencies_per_plugin {
        return Err(PluginRegistryError::DependencyCapacity);
    }
    if plugin.contributions.len() > config.max_contributions_per_plugin {
        return Err(PluginRegistryError::ContributionCapacity);
    }
    let mut dependencies = BTreeSet::new();
    if plugin
        .dependencies
        .iter()
        .any(|dependency| !dependencies.insert(*dependency))
    {
        return Err(PluginRegistryError::InvalidPlugin);
    }
    let mut contributions = BTreeSet::new();
    for contribution in &plugin.contributions {
        if contribution.stable_id == 0
            || contribution.version == 0
            || !valid_name(&contribution.name)
            || contribution.required_capability == Some(0)
        {
            return Err(PluginRegistryError::InvalidContribution);
        }
        if !contributions.insert((contribution.kind, contribution.stable_id)) {
            return Err(PluginRegistryError::DuplicateContribution);
        }
    }
    Ok(())
}

fn topological_order(
    plugins: &BTreeMap<PluginId, PluginDescriptor>,
) -> Result<Vec<PluginId>, PluginRegistryError> {
    let mut incoming = plugins
        .keys()
        .map(|id| (*id, 0_usize))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = plugins
        .keys()
        .map(|id| (*id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for plugin in plugins.values() {
        for dependency in &plugin.dependencies {
            *incoming
                .get_mut(&plugin.id)
                .ok_or(PluginRegistryError::UnknownDependency)? += 1;
            outgoing
                .get_mut(dependency)
                .ok_or(PluginRegistryError::UnknownDependency)?
                .insert(plugin.id);
        }
    }
    let mut ready = incoming
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(*id))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(plugins.len());
    while let Some(id) = ready.pop_first() {
        ordered.push(id);
        for dependent in &outgoing[&id] {
            let count = incoming.get_mut(dependent).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.insert(*dependent);
            }
        }
    }
    if ordered.len() != plugins.len() {
        return Err(PluginRegistryError::DependencyCycle);
    }
    Ok(ordered)
}

fn descriptor_bytes(plugin: &PluginDescriptor) -> Option<usize> {
    let mut bytes = 48_usize
        .checked_add(plugin.name.len())?
        .checked_add(plugin.dependencies.len().checked_mul(8)?)?
        .checked_add(plugin.required_capabilities.len().checked_mul(8)?)?;
    for contribution in &plugin.contributions {
        bytes = bytes
            .checked_add(40)?
            .checked_add(contribution.name.len())?;
    }
    Some(bytes)
}

fn registry_fingerprint(
    ordered: &[PluginId],
    plugins: &BTreeMap<PluginId, PluginDescriptor>,
) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for id in ordered {
        let plugin = &plugins[id];
        hash = mix(hash, &plugin.id.0.to_le_bytes());
        hash = mix(hash, &plugin.version.to_le_bytes());
        hash = mix(hash, plugin.name.as_bytes());
        for dependency in &plugin.dependencies {
            hash = mix(hash, &dependency.0.to_le_bytes());
        }
        for capability in &plugin.required_capabilities {
            hash = mix(hash, &capability.to_le_bytes());
        }
        for contribution in &plugin.contributions {
            hash = mix(hash, &[contribution.kind as u8]);
            hash = mix(hash, &contribution.stable_id.to_le_bytes());
            hash = mix(hash, &contribution.version.to_le_bytes());
            hash = mix(hash, contribution.name.as_bytes());
            hash = mix(
                hash,
                &contribution.required_capability.unwrap_or(0).to_le_bytes(),
            );
        }
    }
    hash
}

fn mix(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
