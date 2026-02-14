import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

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

  const instanceCards = useMemo(() => {
    if (instances.length > 0) return instances;
    return [
      {
        id: "demo-1",
        name: "Survival Coop",
        minecraft_version: "1.20.1",
        loader_type: "fabric" as const,
        loader_version: "0.16.5",
        total_size_bytes: 3_742_314_496,
      },
    ];
  }, [instances]);

  const enterEditMode = () => {
    if (!selectedInstance) return;
    setEditingInstance(selectedInstance);
    setShowInstancePanel(false);
  };

  const launchInstance = () => {
    if (!selectedInstance) return;
    setEditingInstance(selectedInstance);
    setActiveEditSection("Ejecucion");
    setShowInstancePanel(false);
  };

  const onSelectInstance = (instance: InstanceInfo) => {
    setSelectedInstance(instance);
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
        <header className="section-header">
          <h1>Mis Instancias</h1>
          <p>Panel ampliado para visualizar todas las instancias y sus acciones.</p>
        </header>

        <div className="instances-compact-toolbar" onClick={(event) => event.stopPropagation()}>
          <span>Gestion de Instancias</span>
          <button type="button" onClick={() => setAppMode("create")}>Crear instancia</button>
        </div>

        <div className={`instances-workspace ${showInstancePanel ? "with-panel" : ""}`}>
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
          </div>

          {showInstancePanel && selectedInstance && (
            <aside className="instance-right-panel" onClick={(event) => event.stopPropagation()}>
              <h3>{selectedInstance.name}</h3>
              {INSTANCE_ACTIONS.map((action) => (
                <button
                  key={action}
                  type="button"
                  onClick={
                    action === "Editar" ? enterEditMode : action === "Iniciar" ? launchInstance : undefined
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
            <button type="button" aria-label="Atras">←</button>
            <button type="button" aria-label="Adelante">→</button>
            <div className="brand">Launcher Principal</div>
          </div>
          <div className="topbar-info">Creando nueva instancia</div>
        </header>
        <div className="create-layout">
          <aside className="create-left-sidebar">
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
          <aside className="create-right-sidebar">
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
            <button type="button" aria-label="Atras">←</button>
            <button type="button" aria-label="Adelante">→</button>
            <div className="brand">Launcher Principal</div>
          </div>
          <div className="topbar-info">Editando: {editingInstance.name}</div>
        </header>
        <div className="edit-layout" onClick={(event) => event.stopPropagation()}>
          <aside className="edit-left-sidebar">
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
                <p>[00:00] Preparando directorios de instancia...</p>
                <p>[00:01] Descargando dependencias y librerías base...</p>
                <p>[00:03] Instalando loader y verificando assets...</p>
                <p>[00:04] Aplicando argumentos de JVM y de lanzamiento...</p>
                <p>[00:06] Iniciando proceso del juego...</p>
                <p>[00:07] [LIVE] Minecraft inicializado correctamente.</p>
              </div>
            ) : (
              <p>Vista completa de la instancia. Todo lo demás está oculto, excepto la barra superior principal.</p>
            )}
          </main>
          <aside className="edit-right-sidebar">
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
          <button type="button" aria-label="Atras">←</button>
          <button type="button" aria-label="Adelante">→</button>
          <div className="brand">Launcher Principal</div>
        </div>
        <div className="topbar-info">Barra principal superior</div>
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
