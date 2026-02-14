import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./styles/app/base.css";
import "./styles/app/layout.css";
import "./styles/app/components.css";
import "./styles/app/pages.css";

type LoaderType = "vanilla" | "forge" | "fabric" | "neoforge" | "quilt";
type TopSection = "menu" | "instances" | "news" | "explorer" | "servers" | "community" | "global-settings";
type InstanceAction = "Iniciar" | "Forzar Cierre" | "Editar" | "Cambiar Grupo" | "Carpeta" | "Exportar" | "Copiar" | "Borrar" | "Crear Atajo";
type EditSection = "Ejecucion" | "Version" | "Mods" | "ResourcePacks" | "Shader Packs" | "Notas" | "Mundos" | "Servidores" | "Capturas" | "Configuracion" | "Otros Registros";
type AppMode = "main" | "create";

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
  const [launchProgress, setLaunchProgress] = useState<LaunchProgressEvent | null>(null);
  const [launchLogs, setLaunchLogs] = useState<LaunchLogEvent[]>([]);
  const [launchError, setLaunchError] = useState<string | null>(null);
  const [instanceSearch, setInstanceSearch] = useState("");

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
                  onClick={
                    action === "Editar" ? enterEditMode : action === "Iniciar" ? () => void launchInstance() : undefined
                  }
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
            <button type="button" aria-label="Atras" className="arrow-button">←</button>
            <button type="button" aria-label="Adelante" className="arrow-button">→</button>
            <div className="brand">Launcher Principal</div>
          </div>
          <div className="topbar-info">Creando nueva instancia</div>
        </header>
        <div className="create-layout">
          <aside className="create-left-sidebar compact-sidebar">
            {CREATE_SECTIONS.map((section) => (
              <button
                key={section}
                type="button"
                className={activeCreateSection === section ? "active" : ""}
                onClick={() => setActiveCreateSection(section)}
              >
                {section}
              </button>
            ))}
          </aside>
          <main className="create-main-content">
            <h2>Crear instancia - {activeCreateSection}</h2>
            <p>Esta vista ocupa la pantalla completa (excepto la barra principal superior).</p>
          </main>
          <aside className="create-right-sidebar compact-sidebar">
            <h3>Panel de {activeCreateSection}</h3>
            <button type="button" onClick={() => setAppMode("main")}>Cancelar</button>
            <button type="button">Guardar borrador</button>
            <button type="button">Crear instancia</button>
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
              <div className="execution-log">
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
        <div className="topbar-info" />
      </header>

      <nav className="topbar-secondary" onClick={(event) => event.stopPropagation()}>
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
      </nav>

      <main className="content-wrap">{renderSectionPage()}</main>
    </div>
  );
}

export default App;
