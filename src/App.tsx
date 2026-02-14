import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type LoaderType = "vanilla" | "forge" | "fabric" | "neoforge" | "quilt";
type TopSection = "menu" | "instances" | "news" | "explorer" | "servers" | "community" | "global-settings";
type InstanceAction = "Iniciar" | "Forzar Cierre" | "Editar" | "Cambiar Grupo" | "Carpeta" | "Exportar" | "Copiar" | "Borrar" | "Crear Atajo";
type EditSection = "Ejecucion" | "Version" | "Mods" | "ResourcePacks" | "Shader Packs" | "Notas" | "Mundos" | "Servidores" | "Capturas" | "Configuracion" | "Otros Registros";

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
      <section className="full-section-page instances-page">
        <header className="section-header">
          <h1>Mis Instancias</h1>
          <p>Panel completo para visualizar todas las instancias creadas.</p>
        </header>
        <div className="instance-grid">
          {instanceCards.map((instance) => (
            <article
              key={instance.id}
              className={`instance-card ${selectedInstance?.id === instance.id ? "active" : ""}`}
              onClick={() => onSelectInstance(instance)}
            >
              <div className="instance-cover">IMG</div>
              <div className="instance-meta">
                <h3>{instance.name}</h3>
                <ul>
                  <li><strong>Version MC:</strong> {instance.minecraft_version}</li>
                  <li><strong>Loader:</strong> {prettyLoader(instance.loader_type)}</li>
                  <li><strong>Version Loader:</strong> {instance.loader_version ?? "N/A"}</li>
                  <li><strong>Autor:</strong> Usuario Local</li>
                  <li><strong>Titulo:</strong> {instance.name}</li>
                  <li><strong>Peso:</strong> {formatBytes(instance.total_size_bytes)}</li>
                </ul>
              </div>
            </article>
          ))}
        </div>
      </section>
    );
  };

  if (editingInstance) {
    return (
      <div className="app-shell" onClick={() => setEditingInstance(null)}>
        <header className="topbar-primary">
          <div className="brand">Launcher Principal</div>
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
            <p>Vista completa de la instancia. Todo lo demás está oculto, excepto la barra superior principal.</p>
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
        <div className="brand">Launcher Principal</div>
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

      <main className="content-wrap" onClick={(event) => event.stopPropagation()}>
        {renderSectionPage()}
      </main>

      {showInstancePanel && selectedInstance && activeSection === "instances" && (
        <aside className="instance-left-panel" onClick={(event) => event.stopPropagation()}>
          <h3>{selectedInstance.name}</h3>
          {INSTANCE_ACTIONS.map((action) => (
            <button key={action} type="button" onClick={action === "Editar" ? enterEditMode : undefined}>
              {action}
            </button>
          ))}
        </aside>
      )}
    </div>
  );
}

export default App;
