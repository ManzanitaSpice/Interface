import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./styles/app/base.css";
import "./styles/app/layout.css";
import "./styles/app/topbar.css";
import "./styles/app/buttons.css";
import "./styles/app/sidebars.css";
import "./styles/app/instances.css";
import "./styles/app/settings.css";

type LoaderType = "vanilla" | "forge" | "fabric" | "neoforge" | "quilt";
type TopSection = "menu" | "instances" | "news" | "explorer" | "servers" | "community" | "global-settings";
type InstanceAction = "Iniciar" | "Forzar Cierre" | "Editar" | "Cambiar Grupo" | "Carpeta" | "Exportar" | "Copiar" | "Borrar" | "Crear Atajo";
type EditSection = "Ejecucion" | "Version" | "Mods" | "ResourcePacks" | "Shader Packs" | "Notas" | "Mundos" | "Servidores" | "Capturas" | "Configuracion" | "Otros Registros";
type AppMode = "main" | "create";
type InstanceConfigTab = "General" | "Java" | "Ajustes" | "Comandos Personalizados" | "Variables de entorno";

interface InstanceInfo {
  id: string;
  name: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  total_size_bytes: number;
}

interface LaunchProgressEvent {
  id: string;
  value: number;
  stage: string;
  state: "idle" | "running" | "done" | "error";
}

interface LaunchLogEvent {
  id: string;
  level: "info" | "warn" | "error";
  message: string;
}

interface MinecraftVersionEntry {
  id: string;
  release_time: string;
  version_type: string;
}

type MinecraftVersionFilter = "all" | "release" | "snapshot" | "old_beta" | "old_alpha" | "experimental";

const SECTION_LABELS: { key: TopSection; label: string }[] = [
  { key: "menu", label: "Menu" },
  { key: "instances", label: "Mis Instancias" },
  { key: "news", label: "Noticias" },
  { key: "explorer", label: "Explorador" },
  { key: "servers", label: "Servidores" },
  { key: "community", label: "Comunidad" },
  { key: "global-settings", label: "Configuracion Global" },
];

const INSTANCE_ACTIONS: InstanceAction[] = [
  "Iniciar",
  "Forzar Cierre",
  "Editar",
  "Cambiar Grupo",
  "Carpeta",
  "Exportar",
  "Copiar",
  "Borrar",
  "Crear Atajo",
];

const EDIT_SECTIONS: EditSection[] = [
  "Ejecucion",
  "Version",
  "Mods",
  "ResourcePacks",
  "Shader Packs",
  "Notas",
  "Mundos",
  "Servidores",
  "Capturas",
  "Configuracion",
  "Otros Registros",
];

const CREATE_SECTIONS = [
  "Base",
  "Version",
  "Loader",
  "Java",
  "Memoria",
  "Mods",
  "Recursos",
  "Revision",
] as const;

const LOADER_CHOICES: { value: LoaderType | "liteloader"; label: string; supported: boolean }[] = [
  { value: "vanilla", label: "Vanilla", supported: true },
  { value: "neoforge", label: "NeoForge", supported: true },
  { value: "forge", label: "Forge", supported: true },
  { value: "fabric", label: "Fabric", supported: true },
  { value: "quilt", label: "Quilt", supported: true },
  { value: "liteloader", label: "LiteLoader", supported: false },
];

const INSTANCE_CONFIG_TABS: InstanceConfigTab[] = [
  "General",
  "Java",
  "Ajustes",
  "Comandos Personalizados",
  "Variables de entorno",
];

const formatBytes = (bytes: number) => {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const power = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** power).toFixed(power === 0 ? 0 : 2)} ${units[power]}`;
};

const prettyLoader = (loader: LoaderType) => loader.charAt(0).toUpperCase() + loader.slice(1);

function App() {
  const [activeSection, setActiveSection] = useState<TopSection>("instances");
  const [instances, setInstances] = useState<InstanceInfo[]>([]);
  const [selectedInstance, setSelectedInstance] = useState<InstanceInfo | null>(null);
  const [showInstancePanel, setShowInstancePanel] = useState(false);
  const [editingInstance, setEditingInstance] = useState<InstanceInfo | null>(null);
  const [activeEditSection, setActiveEditSection] = useState<EditSection>("Ejecucion");
  const [appMode, setAppMode] = useState<AppMode>("main");
  const [activeCreateSection, setActiveCreateSection] = useState<(typeof CREATE_SECTIONS)[number]>("Base");
  const [activeInstanceConfigTab, setActiveInstanceConfigTab] = useState<InstanceConfigTab>("General");
  const [launchProgress, setLaunchProgress] = useState<LaunchProgressEvent | null>(null);
  const [launchLogs, setLaunchLogs] = useState<LaunchLogEvent[]>([]);
  const [launchError, setLaunchError] = useState<string | null>(null);
  const [instanceSearch, setInstanceSearch] = useState("");
  const [showProfileMenu, setShowProfileMenu] = useState(false);
  const [createSectionHistory, setCreateSectionHistory] = useState<(typeof CREATE_SECTIONS)[number][]>(["Base"]);
  const [createHistoryIndex, setCreateHistoryIndex] = useState(0);
  const [minecraftVersions, setMinecraftVersions] = useState<MinecraftVersionEntry[]>([]);
  const [minecraftFilter, setMinecraftFilter] = useState<MinecraftVersionFilter>("all");
  const [selectedMinecraftVersion, setSelectedMinecraftVersion] = useState<string | null>(null);
  const [selectedLoaderType, setSelectedLoaderType] = useState<LoaderType | null>(null);
  const [loaderVersions, setLoaderVersions] = useState<string[]>([]);
  const [selectedLoaderVersion, setSelectedLoaderVersion] = useState<string | null>(null);
  const [newInstanceName, setNewInstanceName] = useState("");
  const [createInProgress, setCreateInProgress] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const executionLogRef = useRef<HTMLDivElement | null>(null);
  const profileMenuRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const loadInstances = async () => {
      try {
        const saved = await invoke<InstanceInfo[]>("list_instances");
        setInstances(saved);
      } catch {
        setInstances([]);
      }
    };
    void loadInstances();
  }, []);

  useEffect(() => {
    const loadMinecraftVersions = async () => {
      try {
        const versions = await invoke<MinecraftVersionEntry[]>("get_minecraft_versions_detailed");
        setMinecraftVersions(versions);
      } catch {
        setMinecraftVersions([]);
      }
    };

    void loadMinecraftVersions();
  }, []);

  useEffect(() => {
    if (!selectedMinecraftVersion || !selectedLoaderType || selectedLoaderType === "vanilla") {
      setLoaderVersions([]);
      setSelectedLoaderVersion(selectedLoaderType === "vanilla" ? "integrado" : null);
      return;
    }

    const loadLoaderVersions = async () => {
      try {
        const versions = await invoke<string[]>("get_loader_versions", {
          loaderType: selectedLoaderType,
          minecraftVersion: selectedMinecraftVersion,
        });
        setLoaderVersions(versions);
        setSelectedLoaderVersion(versions[0] ?? null);
      } catch {
        setLoaderVersions([]);
        setSelectedLoaderVersion(null);
      }
    };

    void loadLoaderVersions();
  }, [selectedLoaderType, selectedMinecraftVersion]);

  useEffect(() => {
    let mounted = true;
    const listeners: UnlistenFn[] = [];

    const setupListeners = async () => {
      const unlistenProgress = await listen<LaunchProgressEvent>("instance-launch-progress", (event) => {
        if (!mounted) return;
        if (selectedInstance && event.payload.id !== selectedInstance.id) return;
        setLaunchProgress(event.payload);
      });

      const unlistenLog = await listen<LaunchLogEvent>("instance-launch-log", (event) => {
        if (!mounted) return;
        if (selectedInstance && event.payload.id !== selectedInstance.id) return;
        setLaunchLogs((prev) => [...prev.slice(-100), event.payload]);
      });

      listeners.push(unlistenProgress, unlistenLog);
    };

    void setupListeners();

    return () => {
      mounted = false;
      listeners.forEach((unlisten) => unlisten());
    };
  }, [selectedInstance]);

  useEffect(() => {
    if (!executionLogRef.current) return;
    executionLogRef.current.scrollTop = executionLogRef.current.scrollHeight;
  }, [launchLogs, launchProgress, launchError]);

  useEffect(() => {
    const onDocumentClick = (event: MouseEvent) => {
      if (!profileMenuRef.current) return;
      if (profileMenuRef.current.contains(event.target as Node)) return;
      setShowProfileMenu(false);
    };

    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (showProfileMenu) {
        setShowProfileMenu(false);
        return;
      }
      if (showInstancePanel) {
        setShowInstancePanel(false);
        return;
      }
      if (editingInstance) {
        setEditingInstance(null);
        return;
      }
      if (appMode === "create") {
        setAppMode("main");
      }
    };

    window.addEventListener("click", onDocumentClick);
    window.addEventListener("keydown", onEscape);
    return () => {
      window.removeEventListener("click", onDocumentClick);
      window.removeEventListener("keydown", onEscape);
    };
  }, [appMode, editingInstance, showInstancePanel, showProfileMenu]);

  const instanceCards = useMemo(() => {
    const query = instanceSearch.trim().toLowerCase();
    if (!query) return instances;

    return instances.filter((instance) =>
      [instance.name, instance.minecraft_version, prettyLoader(instance.loader_type)]
        .join(" ")
        .toLowerCase()
        .includes(query),
    );
  }, [instances, instanceSearch]);

  const enterEditMode = () => {
    if (!selectedInstance) return;
    setEditingInstance(selectedInstance);
    setShowInstancePanel(false);
  };

  const reloadInstances = async () => {
    const saved = await invoke<InstanceInfo[]>("list_instances");
    setInstances(saved);
    if (selectedInstance) {
      const updated = saved.find((instance) => instance.id === selectedInstance.id) ?? null;
      setSelectedInstance(updated);
      if (!updated) {
        setShowInstancePanel(false);
        setEditingInstance(null);
      }
    }
  };

  const copyInstance = async () => {
    if (!selectedInstance) return;
    await invoke("clone_instance", { id: selectedInstance.id });
    await reloadInstances();
  };

  const deleteSelectedInstance = async () => {
    if (!selectedInstance) return;
    const confirmed = window.confirm(`¿Borrar completamente la instancia ${selectedInstance.name} y todos sus archivos?`);
    if (!confirmed) return;
    await invoke("delete_instance", { id: selectedInstance.id });
    await reloadInstances();
  };

  const handleInstanceAction = async (action: InstanceAction) => {
    if (!selectedInstance) return;
    if (action === "Editar") {
      enterEditMode();
      return;
    }
    if (action === "Iniciar") {
      await launchInstance();
      return;
    }
    if (action === "Forzar Cierre") {
      await invoke("force_close_instance", { id: selectedInstance.id });
      return;
    }
    if (action === "Carpeta") {
      await invoke("open_instance_folder", { id: selectedInstance.id });
      return;
    }
    if (action === "Copiar") {
      await copyInstance();
      return;
    }
    if (action === "Borrar") {
      await deleteSelectedInstance();
    }
  };

  const goBackCreateSection = () => {
    if (createHistoryIndex <= 0) return;
    const nextIndex = createHistoryIndex - 1;
    setCreateHistoryIndex(nextIndex);
    setActiveCreateSection(createSectionHistory[nextIndex]);
  };

  const goForwardCreateSection = () => {
    if (createHistoryIndex >= createSectionHistory.length - 1) return;
    const nextIndex = createHistoryIndex + 1;
    setCreateHistoryIndex(nextIndex);
    setActiveCreateSection(createSectionHistory[nextIndex]);
  };

  const selectCreateSection = (section: (typeof CREATE_SECTIONS)[number]) => {
    setActiveCreateSection(section);
    setCreateSectionHistory((prev) => {
      const compact = prev.slice(0, createHistoryIndex + 1);
      if (compact[compact.length - 1] === section) return compact;
      const next = [...compact, section];
      setCreateHistoryIndex(next.length - 1);
      return next;
    });
  };

  const launchInstance = async () => {
    if (!selectedInstance) return;
    setLaunchError(null);
    setLaunchLogs([]);
    setLaunchProgress({
      id: selectedInstance.id,
      value: 0,
      stage: "Solicitando lanzamiento",
      state: "running",
    });

    try {
      await invoke("launch_instance", { id: selectedInstance.id });
      setEditingInstance(selectedInstance);
      setActiveEditSection("Ejecucion");
      setShowInstancePanel(false);
    } catch (error) {
      const errorMessage = typeof error === "string" ? error : "No se pudo iniciar la instancia.";
      setLaunchError(errorMessage);
      setLaunchProgress({
        id: selectedInstance.id,
        value: 100,
        stage: "Error al iniciar",
        state: "error",
      });
    }
  };

  const filteredMinecraftVersions = useMemo(() => {
    if (minecraftFilter === "all") return minecraftVersions;
    if (minecraftFilter === "experimental") {
      return minecraftVersions.filter((entry) => entry.version_type !== "release");
    }
    return minecraftVersions.filter((entry) => entry.version_type === minecraftFilter);
  }, [minecraftFilter, minecraftVersions]);

  const createInstanceNow = async () => {
    if (!selectedMinecraftVersion || !selectedLoaderType) {
      setCreateError("Selecciona versión de Minecraft y loader.");
      return;
    }
    if (selectedLoaderType !== "vanilla" && !selectedLoaderVersion) {
      setCreateError("No hay versión de loader compatible.");
      return;
    }
    const name = newInstanceName.trim();
    if (!name) {
      setCreateError("Escribe un nombre de instancia.");
      return;
    }

    setCreateInProgress(true);
    setCreateError(null);
    try {
      await invoke("create_instance", {
        payload: {
          name,
          minecraftVersion: selectedMinecraftVersion,
          loaderType: selectedLoaderType,
          loaderVersion: selectedLoaderType === "vanilla" ? null : selectedLoaderVersion,
        },
      });
      await reloadInstances();
      setAppMode("main");
      setActiveSection("instances");
    } catch (error) {
      setCreateError(typeof error === "string" ? error : "No se pudo crear la instancia.");
    } finally {
      setCreateInProgress(false);
    }
  };

  const onSelectInstance = (instance: InstanceInfo) => {
    setSelectedInstance(instance);
    setLaunchError(null);
    setLaunchLogs([]);
    setLaunchProgress(null);
    setShowInstancePanel(true);
  };

  const renderSectionPage = () => {
    if (activeSection !== "instances") {
      const label = SECTION_LABELS.find((section) => section.key === activeSection)?.label;
      return (
        <section className="full-section-page">
          <h1>{label}</h1>
          <p>Esta sección ahora ocupa una página completa. Aquí irá su contenido dedicado.</p>
        </section>
      );
    }

    return (
      <section className="full-section-page instances-page" onClick={() => setShowInstancePanel(false)}>
        <div className="instances-toolbar" onClick={(event) => event.stopPropagation()}>
          <div className="instances-toolbar-left">
            <button type="button" onClick={() => setAppMode("create")}>Crear instancia</button>
            <button type="button">Importar</button>
            <button type="button">Crear grupo</button>
          </div>
          <div className="instances-toolbar-right">
            <label htmlFor="instances-search" className="sr-only">Buscar instancias</label>
            <input
              id="instances-search"
              type="search"
              placeholder="Buscar instancias"
              value={instanceSearch}
              onChange={(event) => setInstanceSearch(event.target.value)}
            />
            <button type="button" aria-label="Filtro rapido">Filtros</button>
            <button type="button" aria-label="Ordenar instancias">Ordenar</button>
            <button type="button" aria-label="Vista en cuadricula">Vista</button>
            <button type="button" aria-label="Acciones masivas">Mas</button>
          </div>
        </div>

        <div className="instances-workspace">
          <div className="instance-grid" onClick={(event) => event.stopPropagation()}>
            {instanceCards.map((instance) => {
              const tooltipText = `Version MC: ${instance.minecraft_version}\nLoader: ${prettyLoader(instance.loader_type)} ${instance.loader_version ?? "N/A"}\nAutor: Usuario Local\nPeso: ${formatBytes(instance.total_size_bytes)}`;
              return (
                <article
                  key={instance.id}
                  className={`instance-card ${selectedInstance?.id === instance.id ? "active" : ""}`}
                  onClick={() => onSelectInstance(instance)}
                >
                  <div className="instance-cover">IMG</div>
                  <div className="instance-meta">
                    <h3>{instance.name}</h3>
                    <div className="instance-extra-tooltip" tabIndex={0}>
                      ℹ️
                      <span className="tooltip-bubble">{tooltipText}</span>
                    </div>
                  </div>
                </article>
              );
            })}
            {instanceCards.length === 0 && <p>No hay resultados para la búsqueda actual.</p>}
          </div>

          {showInstancePanel && selectedInstance && (
            <aside className="instance-right-panel" onClick={(event) => event.stopPropagation()}>
              <h3>{selectedInstance.name}</h3>
              {INSTANCE_ACTIONS.map((action) => (
                <button
                  key={action}
                  type="button"
                  onClick={() => void handleInstanceAction(action)}
                >
                  {action}
                </button>
              ))}
            </aside>
          )}
        </div>
      </section>
    );
  };

  if (appMode === "create") {
    return (
      <div className="app-shell">
        <header className="topbar-primary">
          <div className="topbar-left-controls">
            <button type="button" aria-label="Atras" className="arrow-button" onClick={goBackCreateSection}>←</button>
            <button type="button" aria-label="Adelante" className="arrow-button" onClick={goForwardCreateSection}>→</button>
            <div className="brand">Launcher Principal</div>
          </div>
          <div className="topbar-info">Creando nueva instancia</div>
        </header>
        <div className="create-layout create-layout-wide">
          <aside className="create-left-sidebar compact-sidebar">
            {CREATE_SECTIONS.map((section) => (
              <button
                key={section}
                type="button"
                className={activeCreateSection === section ? "active" : ""}
                onClick={() => selectCreateSection(section)}
              >
                {section}
              </button>
            ))}
          </aside>
          <main className="create-main-content create-main-grid">
            <section className="create-block">
              <header><h2>Bloque 1 · Versiones Minecraft</h2></header>
              <div className="create-block-body">
                <div className="create-list-wrap">
                  <table className="version-table">
                    <thead><tr><th>Version</th><th>Fecha de lanzado</th><th>Tipo</th></tr></thead>
                    <tbody>
                      {filteredMinecraftVersions.map((entry) => (
                        <tr key={entry.id} className={selectedMinecraftVersion === entry.id ? "selected" : ""} onClick={() => setSelectedMinecraftVersion(entry.id)}>
                          <td>{entry.id}</td>
                          <td>{new Date(entry.release_time).toLocaleDateString("es-ES")}</td>
                          <td>{entry.version_type}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                <aside className="block-sidebar">
                  <h3>Filtro</h3>
                  {[["all","Versiones"],["release","Reales"],["snapshot","Snapshots"],["old_beta","Betas"],["old_alpha","Alfas"],["experimental","Experimentales"]].map(([value, label]) => (
                    <button key={value} type="button" className={minecraftFilter === value ? "active" : ""} onClick={() => setMinecraftFilter(value as MinecraftVersionFilter)}>{label}</button>
                  ))}
                </aside>
              </div>
            </section>
            <section className="create-block">
              <header><h2>Bloque 2 · Loaders</h2></header>
              <div className="create-block-body">
                <div className="create-list-wrap">
                  <table className="version-table">
                    <thead><tr><th>Version</th><th>Compatibilidad</th><th>Estado</th></tr></thead>
                    <tbody>
                      {selectedLoaderType === null ? <tr><td colSpan={3}>Selecciona un loader.</td></tr> : selectedLoaderType === "vanilla" ? <tr className="selected"><td>Integrado</td><td>{selectedMinecraftVersion ?? "-"}</td><td>Recomendado</td></tr> : loaderVersions.length === 0 ? <tr><td colSpan={3}>Sin versiones compatibles.</td></tr> : loaderVersions.map((version, idx) => (
                        <tr key={version} className={selectedLoaderVersion === version ? "selected" : ""} onClick={() => setSelectedLoaderVersion(version)}>
                          <td>{version}</td><td>{selectedMinecraftVersion ?? "-"}</td><td>{idx === 0 ? "Recomendada / Más actual" : "Disponible"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                <aside className="block-sidebar">
                  <h3>Cargador de mods</h3>
                  {LOADER_CHOICES.map((loader) => (
                    <button key={loader.value} type="button" className={selectedLoaderType === loader.value ? "active" : ""} disabled={!loader.supported} onClick={() => loader.supported && setSelectedLoaderType(loader.value as LoaderType)}>
                      {loader.label}
                    </button>
                  ))}
                </aside>
              </div>
            </section>
          </main>
          <aside className="create-right-sidebar compact-sidebar">
            <h3>Crear instancia</h3>
            <label htmlFor="instance-name">Nombre</label>
            <input id="instance-name" type="text" value={newInstanceName} onChange={(event) => setNewInstanceName(event.target.value)} placeholder="Mi instancia" />
            <p>MC: {selectedMinecraftVersion ?? "Sin seleccionar"}</p>
            <p>Loader: {selectedLoaderType ? prettyLoader(selectedLoaderType) : "Sin seleccionar"}</p>
            <p>Version loader: {selectedLoaderType === "vanilla" ? "Integrado" : (selectedLoaderVersion ?? "Sin seleccionar")}</p>
            {createError && <p className="execution-error">{createError}</p>}
            <button type="button" onClick={() => setAppMode("main")}>Cancelar</button>
            <button type="button" onClick={() => void createInstanceNow()} disabled={createInProgress}>{createInProgress ? "Creando..." : "Crear instancia"}</button>
          </aside>
        </div>
      </div>
    );
  }


  if (editingInstance) {
    return (
      <div className="app-shell" onClick={() => setEditingInstance(null)}>
        <header className="topbar-primary">
          <div className="topbar-left-controls">
            <button type="button" aria-label="Atras" className="arrow-button">←</button>
            <button type="button" aria-label="Adelante" className="arrow-button">→</button>
            <div className="brand">Launcher Principal</div>
          </div>
          <div className="topbar-info">Editando: {editingInstance.name}</div>
        </header>
        <div className="edit-layout" onClick={(event) => event.stopPropagation()}>
          <aside className="edit-left-sidebar compact-sidebar">
            {EDIT_SECTIONS.map((section) => (
              <button
                key={section}
                type="button"
                className={activeEditSection === section ? "active" : ""}
                onClick={() => setActiveEditSection(section)}
              >
                {section}
              </button>
            ))}
          </aside>
          <main className="edit-main-content">
            <h2>{activeEditSection}</h2>
            {activeEditSection === "Ejecucion" ? (
              <div className="execution-log" ref={executionLogRef}>
                <div className="execution-actions">
                  <button type="button" onClick={() => void launchInstance()}>Iniciar</button>
                  {launchProgress && <span>{launchProgress.stage} ({launchProgress.value}%)</span>}
                </div>
                {launchError && <p className="execution-error">{launchError}</p>}
                {launchLogs.length === 0 ? (
                  <p>Sin logs todavía. Pulsa iniciar para lanzar la instancia real desde backend.</p>
                ) : (
                  launchLogs.map((log, index) => (
                    <p key={`${log.level}-${index}`}>[{log.level.toUpperCase()}] {log.message}</p>
                  ))
                )}
              </div>
            ) : activeEditSection === "Configuracion" ? (
              <section className="instance-settings-panel">
                <header className="instance-settings-header">
                  <h3>Configuracion de instancia</h3>
                  <div className="instance-settings-tabs" role="tablist" aria-label="Configuracion de instancia">
                    {INSTANCE_CONFIG_TABS.map((tab) => (
                      <button
                        key={tab}
                        type="button"
                        role="tab"
                        aria-selected={activeInstanceConfigTab === tab}
                        className={activeInstanceConfigTab === tab ? "active" : ""}
                        onClick={() => setActiveInstanceConfigTab(tab)}
                      >
                        {tab}
                      </button>
                    ))}
                  </div>
                </header>

                {activeInstanceConfigTab === "General" && (
                  <div className="settings-block-grid">
                    {[
                      "Game Windows",
                      "Console",
                      "Window",
                      "Global Datapa Packs",
                      "Game Time",
                      "Default Account",
                      "ModLoaders",
                    ].map((option) => (
                      <article key={option} className="settings-card">
                        <h4>{option}</h4>
                        <p>Opciones rapidas para configurar {option.toLowerCase()}.</p>
                      </article>
                    ))}
                  </div>
                )}

                {activeInstanceConfigTab === "Java" && (
                  <div className="settings-java-layout">
                    <article className="settings-card">
                      <h4>Instalacion de Java</h4>
                      <label htmlFor="java-path">Ruta ejecutable</label>
                      <div className="settings-inline-field">
                        <input id="java-path" type="text" value="C:/Program Files/Java/jdk-21/bin/javaw.exe" readOnly />
                        <button type="button">Detectar Javas</button>
                        <button type="button">Explorar</button>
                      </div>
                    </article>

                    <article className="settings-card">
                      <h4>Memoria</h4>
                      <p>Asignacion actual: 6144 MB de RAM.</p>
                      <input type="range" min={1024} max={16384} step={512} defaultValue={6144} />
                    </article>

                    <article className="settings-card">
                      <h4>Argumentos de Java</h4>
                      <textarea rows={5} defaultValue="-XX:+UseG1GC -XX:+UnlockExperimentalVMOptions" />
                    </article>
                  </div>
                )}

                {activeInstanceConfigTab !== "General" && activeInstanceConfigTab !== "Java" && (
                  <article className="settings-card settings-placeholder">
                    <h4>{activeInstanceConfigTab}</h4>
                    <p>Seccion lista para configurar opciones avanzadas de la instancia.</p>
                  </article>
                )}
              </section>
            ) : (
              <p>Vista completa de la instancia. Todo lo demás está oculto, excepto la barra superior principal.</p>
            )}
          </main>
          <aside className="edit-right-sidebar compact-sidebar">
            <h3>Acciones de {activeEditSection}</h3>
            <button type="button">Accion 1</button>
            <button type="button">Accion 2</button>
            <button type="button">Accion 3</button>
          </aside>
        </div>
      </div>
    );
  }

  return (
    <div className="app-shell" onClick={() => setShowInstancePanel(false)}>
      <header className="topbar-primary">
        <div className="topbar-left-controls">
          <button type="button" aria-label="Atras" className="arrow-button">←</button>
          <button type="button" aria-label="Adelante" className="arrow-button">→</button>
          <div className="brand">Launcher Principal</div>
        </div>
        <div className="topbar-right-controls">
          <div className="topbar-info" />
        </div>
      </header>

      <nav className="topbar-secondary" onClick={(event) => event.stopPropagation()}>
        <div className="secondary-nav-items">
          {SECTION_LABELS.map((section) => (
            <button
              key={section.key}
              type="button"
              className={activeSection === section.key ? "active" : ""}
              onClick={() => {
                setActiveSection(section.key);
                setShowInstancePanel(false);
              }}
            >
              {section.label}
            </button>
          ))}
        </div>

        <div className="topbar-profile" ref={profileMenuRef}>
          <button type="button" onClick={() => setShowProfileMenu((prev) => !prev)}>Perfil</button>
          {showProfileMenu && (
            <div className="profile-menu" role="menu" aria-label="Perfil">
              <button type="button" role="menuitem">Perfil 1</button>
              <button type="button" role="menuitem">Perfil 2</button>
              <button type="button" role="menuitem">Perfil 3</button>
              <button type="button" role="menuitem">Perfil 4</button>
              <button type="button" role="menuitem">Perfil 5</button>
            </div>
          )}
        </div>
      </nav>

      <main className="content-wrap">{renderSectionPage()}</main>
    </div>
  );
}

export default App;
