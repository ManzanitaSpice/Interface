import { useState, useEffect } from "react";
import "./App.css";

// Mock Data
const MINECRAFT_VERSIONS = ["1.20.4", "1.20.1", "1.19.4", "1.19.2", "1.18.2", "1.16.5", "1.12.2"];
const LOADERS = ["Vanilla", "Forge", "Fabric", "NeoForge", "Quilt"];

const LOADER_VERSIONS: Record<string, string[]> = {
  Vanilla: ["Latest Release", "Snapshot"],
  Forge: ["47.2.0 (Recommended)", "47.2.1", "47.1.0"],
  Fabric: ["0.15.3", "0.15.2", "0.14.24"],
  NeoForge: ["20.4.80-beta", "20.4.5"],
  Quilt: ["0.23.1-beta", "0.22.0"],
};

function LoadingScreen() {
  const [progress, setProgress] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => {
      setProgress((prev) => {
        if (prev >= 100) {
          clearInterval(interval);
          return 100;
        }
        return prev + 2; // Speed of loading
      });
    }, 50);

    return () => clearInterval(interval);
  }, []);

  return (
    <div className="loading-screen">
      <h2 className="loading-title">Initializing Launcher...</h2>
      <div className="progress-bar-container">
        <div className="progress-bar-fill" style={{ width: `${progress}%` }}></div>
      </div>
      <p style={{ marginTop: "10px", color: "#888" }}>{progress}%</p>
    </div>
  );
}

function CreateInstancePage() {
  const [selectedVersion, setSelectedVersion] = useState(MINECRAFT_VERSIONS[0]);
  const [selectedLoader, setSelectedLoader] = useState(LOADERS[0]);
  const [availableLoaderVersions, setAvailableLoaderVersions] = useState<string[]>([]);
  const [selectedLoaderVersion, setSelectedLoaderVersion] = useState("");
  const [instanceName, setInstanceName] = useState("");

  // Update loader versions when loader changes
  useEffect(() => {
    const versions = LOADER_VERSIONS[selectedLoader] || [];
    setAvailableLoaderVersions(versions);
    if (versions.length > 0) {
      setSelectedLoaderVersion(versions[0]);
    }
  }, [selectedLoader, selectedVersion]);

  const handleGenerate = (e: React.FormEvent) => {
    e.preventDefault();
    alert(`Generating Instance:
    Name: ${instanceName}
    Minecraft: ${selectedVersion}
    Loader: ${selectedLoader}
    Loader Version: ${selectedLoaderVersion}`);
  };

  return (
    <div className="create-instance-page">
      <h2>Create New Instance</h2>
      <form className="create-instance-form" onSubmit={handleGenerate}>
        <div className="form-group">
          <label>Instance Name</label>
          <input
            type="text"
            placeholder="My New Modpack"
            value={instanceName}
            onChange={(e) => setInstanceName(e.target.value)}
            required
          />
        </div>

        <div className="form-group">
          <label>Minecraft Version</label>
          <select
            value={selectedVersion}
            onChange={(e) => setSelectedVersion(e.target.value)}
          >
            {MINECRAFT_VERSIONS.map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </select>
        </div>

        <div className="form-group">
          <label>Mod Loader</label>
          <select
            value={selectedLoader}
            onChange={(e) => setSelectedLoader(e.target.value)}
          >
            {LOADERS.map((l) => (
              <option key={l} value={l}>
                {l}
              </option>
            ))}
          </select>
        </div>

        <div className="form-group">
          <label>Loader Version</label>
          <select
            value={selectedLoaderVersion}
            onChange={(e) => setSelectedLoaderVersion(e.target.value)}
            disabled={availableLoaderVersions.length === 0}
          >
            {availableLoaderVersions.map((lv) => (
              <option key={lv} value={lv}>
                {lv}
              </option>
            ))}
          </select>
        </div>

        <button type="submit" className="generate-btn">
          Generate Instance
        </button>
      </form>
    </div>
  );
}

function App() {
  const [isLoading, setIsLoading] = useState(true);
  const [currentView, setCurrentView] = useState("home"); // 'home' | 'create-instance'

  // Simulate loading delay
  useEffect(() => {
    const timer = setTimeout(() => {
      setIsLoading(false);
    }, 3000); // 3 seconds loading time
    return () => clearTimeout(timer);
  }, []);

  if (isLoading) {
    return <LoadingScreen />;
  }

  return (
    <div className="app-layout">
      {/* Sidebar */}
      <aside className="sidebar">
        <div className="sidebar-header">Launcher</div>
        <nav>
          <button
            className={`sidebar-btn ${currentView === "home" ? "active" : ""}`}
            onClick={() => setCurrentView("home")}
          >
            Home
          </button>
          <button
            className={`sidebar-btn ${currentView === "create-instance" ? "active" : ""}`}
            onClick={() => setCurrentView("create-instance")}
          >
            Create Instance
          </button>
          <button className="sidebar-btn">Settings</button>
          <button className="sidebar-btn">About</button>
        </nav>
      </aside>

      {/* Main Content */}
      <main className="content-area">
        {currentView === "home" && (
          <div>
            <h2>Welcome Back!</h2>
            <p>Select an instance from the sidebar or create a new one to get started.</p>
          </div>
        )}
        {currentView === "create-instance" && <CreateInstancePage />}
      </main>
    </div>
  );
}

export default App;
