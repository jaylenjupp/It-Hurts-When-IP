import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { platform } from "@tauri-apps/plugin-os";
import { openUrl } from "@tauri-apps/plugin-opener";

const appWindow = getCurrentWindow();

// Set <body data-platform="..."> so CSS can target macOS / Windows differences.
// Tauri's platform() returns "macos", "windows", "linux", etc. We default to
// "macos" on any failure since that's the more conservative path (uses native
// titlebar, no custom chrome to break).
(async () => {
  try {
    const p = await platform();
    document.body.setAttribute("data-platform", p);
  } catch {
    document.body.setAttribute("data-platform", "macos");
  }
})();

// Custom titlebar buttons only exist in the DOM when the platform needs them
// (Windows). The optional chaining means this is a no-op on macOS, which uses
// the native titlebar.
document.getElementById("titlebar-minimize")?.addEventListener("click", () => appWindow.minimize());
document.getElementById("titlebar-close")?.addEventListener("click", () => appWindow.close());

const interfaceSelect = document.getElementById("interface-select");
const statusBar = document.getElementById("status-bar");
const currentIpLabel = document.getElementById("current-ip-label");
const currentSubnetLabel = document.getElementById("current-subnet-label");
const currentGatewayLabel = document.getElementById("current-gateway-label");
const currentDhcpLabel = document.getElementById("current-dhcp-label");

// --- Helper status banner ---
// One line at the top of the window when the privileged helper is missing,
// not running, out of date, a different version, or not usable from this
// account. Checked at launch and after a failed write; never polled. The
// quick-set buttons stay usable whatever it says: a write's own error is the
// truth.

const helperBanner = document.getElementById("helper-banner");
let helperCheckInFlight = false;

function renderHelperBanner(status) {
  const state = status && status.state;
  const show = state && state !== "ok" && status.message;
  helperBanner.textContent = show ? status.message : "";
  helperBanner.classList.toggle("hidden", !show);
  helperBanner.classList.toggle("helper-banner--info", state === "dev_build");
  helperBanner.classList.toggle("helper-banner--warn", Boolean(show) && state !== "dev_build");
  helperBanner.dataset.state = state || "";
}

// At most one check at a time; a failed check leaves the banner as it was.
async function refreshHelperBanner() {
  if (helperCheckInFlight) return;
  helperCheckInFlight = true;
  try {
    renderHelperBanner(await invoke("get_helper_status"));
  } catch (e) {
    console.warn("[helper status check failed]", e);
  } finally {
    helperCheckInFlight = false;
  }
}

// Show the write's own error first (full text), then re-check the helper in
// the background without waiting for it.
function reportWriteError(error) {
  statusBar.textContent = "Error: " + error;
  refreshHelperBanner();
}

document.addEventListener("DOMContentLoaded", () => refreshHelperBanner());

let previousIpInfo = null;
let quickSet1 = null;
let quickSet2 = null;
let quickSet3 = null;
let quickSet4 = null;
let manualWasOpen = false;

async function loadInterfaces() {
  try {
    const interfaces = await invoke("get_interfaces");

    interfaceSelect.innerHTML = "";

    interfaces.forEach((iface) => {
      const option = document.createElement("option");
      option.value = iface;
      option.textContent = iface;
      interfaceSelect.appendChild(option);
    });

    // Restore last used interface if it still exists in the list
    const savedInterface = await invoke("get_last_interface");
    if (savedInterface && interfaces.includes(savedInterface)) {
      interfaceSelect.value = savedInterface;
    }

    await loadIpInfo(interfaceSelect.value);
    statusBar.textContent = "Interfaces loaded";
  } catch (error) {
    statusBar.textContent = "Error loading interfaces: " + error;
  }
}

async function loadIpInfo(iface) {
  try {
    const info = await invoke("get_ip_info", { interface: iface });

    currentIpLabel.textContent = "IP: " + info.ip;
    currentSubnetLabel.textContent = "Subnet: " + info.subnet;
    currentGatewayLabel.textContent = "Gateway: " + info.gateway;
    currentDhcpLabel.textContent = info.is_dhcp ? "Mode: DHCP" : "Mode: Static";

    const linkLocalWarning = document.getElementById("link-local-warning");
    if (info.ip && info.ip.startsWith("169.254")) {
      linkLocalWarning.textContent = "⚠ Link-local address — no DHCP server found";
    } else {
      linkLocalWarning.textContent = "";
    }

    updateActiveButton(info);
    return info;
  } catch (error) {
    statusBar.textContent = "Error reading IP info: " + error;
  }
}

async function loadQuickSets() {
  try {
    const profiles = await invoke("get_quick_sets");
    quickSet1 = profiles.quick_set_1;
    quickSet2 = profiles.quick_set_2;
    quickSet3 = profiles.quick_set_3;
    quickSet4 = profiles.quick_set_4;

    document.getElementById("quick-set-1").querySelector(".btn-title").textContent = quickSet1.name;
    document.getElementById("quick-set-2").querySelector(".btn-title").textContent = quickSet2.name;
    document.getElementById("quick-set-3").querySelector(".btn-title").textContent = quickSet3.name;
    document.getElementById("quick-set-4").querySelector(".btn-title").textContent = quickSet4.name;

    document.getElementById("qs1-subtitle").textContent = quickSet1.ip || "Not configured";
    document.getElementById("qs2-subtitle").textContent = quickSet2.ip || "Not configured";
    document.getElementById("qs3-subtitle").textContent = quickSet3.ip || "Not configured";
    document.getElementById("qs4-subtitle").textContent = quickSet4.ip || "Not configured";
  } catch (error) {
    statusBar.textContent = "Error loading quick sets: " + error;
  }
}

function updateActiveButton(info) {
  const display = document.querySelector(".current-ip-display");

  ["quick-set-1", "quick-set-2", "quick-set-3", "quick-set-4", "prev-ip", "set-dhcp"].forEach(id => {
    document.getElementById(id).classList.remove("active-mode");
  });

  if (info.is_dhcp) {
    document.getElementById("set-dhcp").classList.add("active-mode");
    const isLinkLocal = info.ip && info.ip.startsWith("169.254");
    const hasRealIp = info.ip && info.ip !== "—" && !isLinkLocal;
    display.style.borderLeftColor = hasRealIp ? "#0a84ff" : "#444";
    return;
  }

  const sets = [
    { el: "quick-set-1", data: quickSet1 },
    { el: "quick-set-2", data: quickSet2 },
    { el: "quick-set-3", data: quickSet3 },
    { el: "quick-set-4", data: quickSet4 },
  ];

  for (const set of sets) {
    if (set.data && set.data.ip && info.ip === set.data.ip &&
      info.subnet === set.data.subnet &&
      normalizeGateway(info.gateway) === normalizeGateway(set.data.gateway)) {
      document.getElementById(set.el).classList.add("active-mode");
      display.style.borderLeftColor = "#0a84ff";
      return;
    }
  }

  if (previousIpInfo && info.ip === previousIpInfo.ip &&
    info.subnet === previousIpInfo.subnet &&
    normalizeGateway(info.gateway) === normalizeGateway(previousIpInfo.gateway)) {
    document.getElementById("prev-ip").classList.add("active-mode");
    display.style.borderLeftColor = "#0a84ff";
    return;
  }

  // No match — neutral border
  display.style.borderLeftColor = "#444";
}

// Treat "no gateway" variants as equivalent. macOS may return the gateway as
// "—" when no router is configured; Windows returns "0.0.0.0" (no default
// route). A quick set saved with "0.0.0.0" must still match either state.
// Strict string equality fails that comparison; normalising both sides through
// here doesn't.
function normalizeGateway(g) {
  if (!g) return "";
  const t = String(g).trim();
  if (t === "" || t === "—" || t === "0.0.0.0") return "";
  return t;
}

// Polls get_ip_info until `predicate(info)` is true, or maxAttempts is hit.
//
// Why this exists: on Windows, the privileged service applies IP changes via
// netsh, which returns the instant the change is queued. But our frontend
// reads state back via PowerShell (Get-NetIPAddress / Get-NetIPInterface),
// which queries a cache that lags by ~100-500ms. So an immediate read after a
// write returns stale state, and updateActiveButton highlights the wrong
// button until the user clicks something else.
//
// On macOS, networksetup reads and writes the same store — so the first poll
// always satisfies the predicate and the function returns immediately. No
// platform branching needed.
async function waitForExpectedState(iface, predicate, opts = {}) {
  const maxAttempts = opts.maxAttempts ?? 10;
  const delayMs = opts.delayMs ?? 150;

  let info = null;
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    info = await loadIpInfo(iface);
    if (predicate(info)) return { matched: true, info };
    if (attempt < maxAttempts) {
      await new Promise(r => setTimeout(r, delayMs));
    }
  }
  return { matched: false, info };
}

interfaceSelect.addEventListener("change", () => {
  invoke("save_last_interface", { interface: interfaceSelect.value });
  loadIpInfo(interfaceSelect.value);
});

document.getElementById("set-dhcp").addEventListener("click", async () => {
  const iface = interfaceSelect.value;
  statusBar.textContent = "Setting DHCP...";

  try {
    previousIpInfo = await loadIpInfo(iface);
    document.getElementById("prev-ip-subtitle").textContent = previousIpInfo.ip || "—";

    const result = await invoke("set_dhcp", { interface: iface });
    statusBar.textContent = "Waiting for DHCP assignment...";

    let attempts = 0;
    const maxAttempts = 10;

    const poll = setInterval(async () => {
      attempts++;
      const info = await loadIpInfo(iface);
      const isLinkLocal = info && info.ip && info.ip.startsWith("169.254");
      const hasRealIp = info && info.ip && info.ip !== "—" && !isLinkLocal;

      if (hasRealIp) {
        clearInterval(poll);
        statusBar.textContent = "DHCP assigned: " + info.ip;
      } else if (attempts >= maxAttempts) {
        clearInterval(poll);
        if (isLinkLocal) {
          statusBar.textContent = "No DHCP server found — link-local address assigned";
        } else {
          statusBar.textContent = "DHCP timeout — no IP assigned. You can still set a static IP.";
        }
        await loadIpInfo(iface);
      } else {
        statusBar.textContent = "Waiting for DHCP... (" + attempts + "s)";
      }
    }, 1000);

  } catch (error) {
    reportWriteError(error);
  }
});

document.getElementById("quick-set-1").addEventListener("click", async (e) => {
  if (e.target.closest(".meatball-zone")) return;
  if (!quickSet1 || !quickSet1.ip) {
    statusBar.textContent = "Quick Set 1 needs to be configured first";
    return;
  }
  const iface = interfaceSelect.value;
  statusBar.textContent = "Applying " + quickSet1.name + "...";

  try {
    previousIpInfo = await loadIpInfo(iface);
    const result = await invoke("set_static_ip", {
      interface: iface,
      ip: quickSet1.ip,
      subnet: quickSet1.subnet,
      gateway: quickSet1.gateway,
    });
    document.getElementById("prev-ip-subtitle").textContent = previousIpInfo.ip || "—";
    statusBar.textContent = result;

    await waitForExpectedState(iface, info =>
      info.ip === quickSet1.ip &&
      info.subnet === quickSet1.subnet &&
      normalizeGateway(info.gateway) === normalizeGateway(quickSet1.gateway)
    );

  } catch (error) {
    reportWriteError(error);
  }
});

document.getElementById("quick-set-2").addEventListener("click", async (e) => {
  if (e.target.closest(".meatball-zone")) return;
  if (!quickSet2 || !quickSet2.ip) {
    statusBar.textContent = "Quick Set 2 needs to be configured first";
    return;
  }
  const iface = interfaceSelect.value;
  statusBar.textContent = "Applying " + quickSet2.name + "...";

  try {
    previousIpInfo = await loadIpInfo(iface);
    const result = await invoke("set_static_ip", {
      interface: iface,
      ip: quickSet2.ip,
      subnet: quickSet2.subnet,
      gateway: quickSet2.gateway,
    });
    document.getElementById("prev-ip-subtitle").textContent = previousIpInfo.ip || "—";
    statusBar.textContent = result;

    await waitForExpectedState(iface, info =>
      info.ip === quickSet2.ip &&
      info.subnet === quickSet2.subnet &&
      normalizeGateway(info.gateway) === normalizeGateway(quickSet2.gateway)
    );
    
  } catch (error) {
    reportWriteError(error);
  }
});

document.getElementById("quick-set-3").addEventListener("click", async (e) => {
  if (e.target.closest(".meatball-zone")) return;
  if (!quickSet3 || !quickSet3.ip) {
    statusBar.textContent = "Quick Set 3 needs to be configured first";
    return;
  }
  const iface = interfaceSelect.value;
  statusBar.textContent = "Applying " + quickSet3.name + "...";
  try {
    previousIpInfo = await loadIpInfo(iface);
    document.getElementById("prev-ip-subtitle").textContent = previousIpInfo.ip || "—";
    const result = await invoke("set_static_ip", {
      interface: iface,
      ip: quickSet3.ip,
      subnet: quickSet3.subnet,
      gateway: quickSet3.gateway,
    });
    statusBar.textContent = result;

    await waitForExpectedState(iface, info =>
      info.ip === quickSet3.ip &&
      info.subnet === quickSet3.subnet &&
      normalizeGateway(info.gateway) === normalizeGateway(quickSet3.gateway)
    );
    
  } catch (error) {
    reportWriteError(error);
  }
});

document.getElementById("quick-set-4").addEventListener("click", async (e) => {
  if (e.target.closest(".meatball-zone")) return;
  if (!quickSet4 || !quickSet4.ip) {
    statusBar.textContent = "Quick Set 4 needs to be configured first";
    return;
  }
  const iface = interfaceSelect.value;
  statusBar.textContent = "Applying " + quickSet4.name + "...";
  try {
    previousIpInfo = await loadIpInfo(iface);
    document.getElementById("prev-ip-subtitle").textContent = previousIpInfo.ip || "—";
    const result = await invoke("set_static_ip", {
      interface: iface,
      ip: quickSet4.ip,
      subnet: quickSet4.subnet,
      gateway: quickSet4.gateway,
    });
    statusBar.textContent = result;

    await waitForExpectedState(iface, info =>
      info.ip === quickSet4.ip &&
      info.subnet === quickSet4.subnet &&
      normalizeGateway(info.gateway) === normalizeGateway(quickSet4.gateway)
    );
    
  } catch (error) {
    reportWriteError(error);
  }
});

document.getElementById("prev-ip").addEventListener("click", async () => {
  if (!previousIpInfo) {
    statusBar.textContent = "No previous IP saved yet";
    return;
  }

  const iface = interfaceSelect.value;
  statusBar.textContent = "Restoring previous IP...";

  try {
    const result = await invoke("set_static_ip", {
      interface: iface,
      ip: previousIpInfo.ip,
      subnet: previousIpInfo.subnet,
      gateway: previousIpInfo.gateway,
    });
    statusBar.textContent = result;
   
    await waitForExpectedState(iface, info =>
      info.ip === previousIpInfo.ip &&
      info.subnet === previousIpInfo.subnet &&
      normalizeGateway(info.gateway) === normalizeGateway(previousIpInfo.gateway)
    );

  } catch (error) {
    reportWriteError(error);
  }
});

let activeEditSlot = null;

const editorPanel = document.getElementById("editor-panel");

// Meatball menu toggle
function toggleMenu(menuId, ...otherIds) {
  const menu = document.getElementById(menuId);
  otherIds.forEach(id => document.getElementById(id).classList.remove("visible"));
  menu.classList.toggle("visible");
}

document.getElementById("meatball-qs1").addEventListener("click", (e) => {
  e.stopPropagation();
  toggleMenu("menu-qs1", "menu-qs2", "menu-qs3", "menu-qs4");
});

document.getElementById("meatball-qs2").addEventListener("click", (e) => {
  e.stopPropagation();
  toggleMenu("menu-qs2", "menu-qs1", "menu-qs3", "menu-qs4");
});

document.getElementById("meatball-qs3").addEventListener("click", (e) => {
  e.stopPropagation();
  toggleMenu("menu-qs3", "menu-qs1", "menu-qs2", "menu-qs4");
});

document.getElementById("meatball-qs4").addEventListener("click", (e) => {
  e.stopPropagation();
  toggleMenu("menu-qs4", "menu-qs1", "menu-qs2", "menu-qs3");
});

// Close menus when clicking anywhere else
document.addEventListener("click", () => {
  ["menu-qs1", "menu-qs2", "menu-qs3", "menu-qs4"].forEach(id => {
    document.getElementById(id).classList.remove("visible");
  });
});

// Enter key in manual IP fields triggers apply
["manual-ip", "manual-subnet", "manual-gateway"].forEach(id => {
  document.getElementById(id).addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      document.getElementById("manual-apply").click();
    }
  });
});

// Enter key in editor fields triggers save
["edit-name", "edit-ip", "edit-subnet", "edit-gateway"].forEach(id => {
  document.getElementById(id).addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      document.getElementById("save-edit").click();
    }
  });
});

// Edit options
document.getElementById("edit-qs1").addEventListener("click", () => {
  activeEditSlot = 1;
  document.getElementById("editor-title").textContent = "Edit Quick Set 1";
  document.getElementById("edit-name").value = quickSet1.name;
  document.getElementById("edit-ip").value = quickSet1.ip;
  document.getElementById("edit-subnet").value = quickSet1.subnet;
  document.getElementById("edit-gateway").value = quickSet1.gateway;
  editorPanel.classList.add("visible");
  document.getElementById("menu-qs1").classList.remove("visible");
  manualWasOpen = document.getElementById("manual-body").classList.contains("open");
  document.getElementById("manual-section").style.display = "none";
  document.getElementById("manual-section").style.display = "none";
});

document.getElementById("edit-qs2").addEventListener("click", () => {
  activeEditSlot = 2;
  document.getElementById("editor-title").textContent = "Edit Quick Set 2";
  document.getElementById("edit-name").value = quickSet2.name;
  document.getElementById("edit-ip").value = quickSet2.ip;
  document.getElementById("edit-subnet").value = quickSet2.subnet;
  document.getElementById("edit-gateway").value = quickSet2.gateway;
  editorPanel.classList.add("visible");
  document.getElementById("menu-qs2").classList.remove("visible"); 
  manualWasOpen = document.getElementById("manual-body").classList.contains("open");
  document.getElementById("manual-section").style.display = "none";
  document.getElementById("manual-section").style.display = "none";
});

document.getElementById("edit-qs3").addEventListener("click", () => {
  activeEditSlot = 3;
  document.getElementById("editor-title").textContent = "Edit Quick Set 3";
  document.getElementById("edit-name").value = quickSet3.name;
  document.getElementById("edit-ip").value = quickSet3.ip;
  document.getElementById("edit-subnet").value = quickSet3.subnet;
  document.getElementById("edit-gateway").value = quickSet3.gateway;
  editorPanel.classList.add("visible");
  document.getElementById("menu-qs3").classList.remove("visible");
  manualWasOpen = document.getElementById("manual-body").classList.contains("open");
  document.getElementById("manual-section").style.display = "none";
  document.getElementById("manual-section").style.display = "none";
});

document.getElementById("edit-qs4").addEventListener("click", () => {
  activeEditSlot = 4;
  document.getElementById("editor-title").textContent = "Edit Quick Set 4";
  document.getElementById("edit-name").value = quickSet4.name;
  document.getElementById("edit-ip").value = quickSet4.ip;
  document.getElementById("edit-subnet").value = quickSet4.subnet;
  document.getElementById("edit-gateway").value = quickSet4.gateway;
  editorPanel.classList.add("visible");
  document.getElementById("menu-qs4").classList.remove("visible");
  manualWasOpen = document.getElementById("manual-body").classList.contains("open");
  document.getElementById("manual-section").style.display = "none";
  document.getElementById("manual-section").style.display = "none";
});

// Reset options
document.getElementById("reset-qs1").addEventListener("click", async () => {
  try {
    await invoke("save_quick_set", {
      slot: 1, name: "Quick Set 1", ip: "", subnet: "", gateway: "",
    });
    await loadQuickSets();
    statusBar.textContent = "Quick Set 1 reset";
    document.getElementById("menu-qs1").classList.remove("visible");
  } catch (error) {
    statusBar.textContent = "Error resetting: " + error;
  }
});

document.getElementById("reset-qs2").addEventListener("click", async () => {
  try {
    await invoke("save_quick_set", {
      slot: 2, name: "Quick Set 2", ip: "", subnet: "", gateway: "",
    });
    await loadQuickSets();
    statusBar.textContent = "Quick Set 2 reset";
    document.getElementById("menu-qs2").classList.remove("visible");
  } catch (error) {
    statusBar.textContent = "Error resetting: " + error;
  }
});

document.getElementById("reset-qs3").addEventListener("click", async () => {
  try {
    await invoke("save_quick_set", {
      slot: 3, name: "Quick Set 3", ip: "", subnet: "", gateway: "",
    });
    await loadQuickSets();
    statusBar.textContent = "Quick Set 3 reset";
    document.getElementById("menu-qs3").classList.remove("visible");
  } catch (error) {
    statusBar.textContent = "Error resetting: " + error;
  }
});

document.getElementById("reset-qs4").addEventListener("click", async () => {
  try {
    await invoke("save_quick_set", {
      slot: 4, name: "Quick Set 4", ip: "", subnet: "", gateway: "",
    });
    await loadQuickSets();
    statusBar.textContent = "Quick Set 4 reset";
    document.getElementById("menu-qs4").classList.remove("visible");
  } catch (error) {
    statusBar.textContent = "Error resetting: " + error;
  }
});

document.getElementById("close-editor").addEventListener("click", () => {
  editorPanel.classList.remove("visible");
  document.getElementById("manual-section").style.display = "";
});

function isValidIp(ip) {
  const parts = ip.split(".");
  if (parts.length !== 4) return false;
  return parts.every(part => {
    const num = Number(part);
    return part !== "" && !isNaN(num) && num >= 0 && num <= 255;
  });
}

function validateEditorFields() {
  const nameInput = document.getElementById("edit-name");
  const ipInput = document.getElementById("edit-ip");
  const subnetInput = document.getElementById("edit-subnet");
  const gatewayInput = document.getElementById("edit-gateway");

  [nameInput, ipInput, subnetInput, gatewayInput].forEach(el => {
    el.classList.remove("invalid");
  });

  let valid = true;

  if (!nameInput.value.trim()) {
    nameInput.classList.add("invalid");
    valid = false;
  }

  if (!isValidIp(ipInput.value.trim())) {
    ipInput.classList.add("invalid");
    valid = false;
  }

  if (!isValidIp(subnetInput.value.trim())) {
    subnetInput.classList.add("invalid");
    valid = false;
  }

  if (!isValidIp(gatewayInput.value.trim())) {
    gatewayInput.classList.add("invalid");
    valid = false;
  }

  return valid;
}

document.getElementById("save-edit").addEventListener("click", async () => {
  if (!validateEditorFields()) {
    statusBar.textContent = "Please fix the highlighted fields";
    return;
  }

  const name = document.getElementById("edit-name").value.trim();
  const ip = document.getElementById("edit-ip").value.trim();
  const subnet = document.getElementById("edit-subnet").value.trim();
  const gateway = document.getElementById("edit-gateway").value.trim();

  try {
    await invoke("save_quick_set", {
      slot: activeEditSlot,
      name, ip, subnet, gateway
    });

    await loadQuickSets();
    editorPanel.classList.remove("visible");
    document.getElementById("manual-section").style.display = "";

    statusBar.textContent = "Quick Set " + activeEditSlot + " saved";
  } catch (error) {
    statusBar.textContent = "Error saving: " + error;
  }
});

["edit-name", "edit-ip", "edit-subnet", "edit-gateway"].forEach(id => {
  document.getElementById(id).addEventListener("input", () => {
    document.getElementById(id).classList.remove("invalid");
  });
});

// ... all your event listeners above ...

// Collapsible manual entry toggle
document.getElementById("manual-toggle").addEventListener("click", () => {
  const body = document.getElementById("manual-body");
  const arrow = document.getElementById("collapse-arrow");
  const header = document.getElementById("manual-toggle");
  const isOpening = !body.classList.contains("open");

  body.classList.toggle("open");
  arrow.classList.toggle("open");
  header.classList.toggle("open");
});

// Manual apply validation
function validateManualFields() {
  const ipInput = document.getElementById("manual-ip");
  const subnetInput = document.getElementById("manual-subnet");
  const gatewayInput = document.getElementById("manual-gateway");

  [ipInput, subnetInput, gatewayInput].forEach(el => el.classList.remove("invalid"));

  let valid = true;

  if (!isValidIp(ipInput.value.trim())) {
    ipInput.classList.add("invalid");
    valid = false;
  }

  if (!isValidIp(subnetInput.value.trim())) {
    subnetInput.classList.add("invalid");
    valid = false;
  }

  if (!isValidIp(gatewayInput.value.trim())) {
    gatewayInput.classList.add("invalid");
    valid = false;
  }

  return valid;
}

document.getElementById("manual-apply").addEventListener("click", async () => {
  if (!validateManualFields()) {
    statusBar.textContent = "Please fix the highlighted fields";
    return;
  }

  const iface = interfaceSelect.value;
  const ip = document.getElementById("manual-ip").value.trim();
  const subnet = document.getElementById("manual-subnet").value.trim();
  const gateway = document.getElementById("manual-gateway").value.trim();

  statusBar.textContent = "Applying manual IP...";

  try {
    previousIpInfo = await loadIpInfo(iface);
    document.getElementById("prev-ip-subtitle").textContent = previousIpInfo.ip || "—";

    const result = await invoke("set_static_ip", {
      interface: iface,
      ip, subnet, gateway,
    });
    statusBar.textContent = result;
    
    await waitForExpectedState(iface, info =>
      info.ip === ip &&
      info.subnet === subnet &&
      normalizeGateway(info.gateway) === normalizeGateway(gateway)
    );

  } catch (error) {
    reportWriteError(error);
  }
});

["manual-ip", "manual-subnet", "manual-gateway"].forEach(id => {
  document.getElementById(id).addEventListener("input", () => {
    document.getElementById(id).classList.remove("invalid");
  });
});

// --- Update modal ---

const updateModal = {
  overlay: null,
  current: null,
  latest: null,
  closeBtn: null,
  changelogBtn: null,
  downloadBtn: null,

  init() {
    this.overlay = document.getElementById("update-modal-overlay");
    this.current = document.getElementById("update-modal-current");
    this.latest = document.getElementById("update-modal-latest");
    this.closeBtn = document.getElementById("update-modal-close");
    this.changelogBtn = document.getElementById("update-modal-changelog");
    this.downloadBtn = document.getElementById("update-modal-download");

    this.closeBtn.addEventListener("click", () => this.dismiss());
    this.changelogBtn.addEventListener("click", () => {
      if (this._releaseUrl) openUrl(this._releaseUrl);
    });
    this.downloadBtn.addEventListener("click", () => {
      if (this._downloadUrl) {
        openUrl(this._downloadUrl);
      } else if (this._releaseUrl) {
        openUrl(this._releaseUrl);
      }
    });
  },

  show(info) {
    this.current.textContent = "v" + info.current_version;
    this.latest.textContent = "v" + info.latest_version;
    this._releaseUrl = info.release_url;
    this._downloadUrl = info.download_url;

    // Hide the Download button if there's no installer asset attached
    this.downloadBtn.style.display = info.download_url ? "" : "none";

    this.overlay.classList.remove("hidden");
  },

  async dismiss() {
    this.overlay.classList.add("hidden");
    try {
      await window.__TAURI__.core.invoke("dismiss_update_prompt");
    } catch (e) {
      console.error("[dismiss_update_prompt failed]", e);
    }
  },
};

// --- Uninstall modal (rescue script) ---
//
// Opened from the app menu ("Uninstall It Hurts When IP…") and Settings →
// Maintenance; both only open this modal in the loading state.
// It shows the rescue script's plan (read-only), lets the user tick Manual
// services to switch to DHCP and/or restoring the recorded settings, re-plans
// after every change so the list shown is what will run, and runs the
// uninstall only after the red confirm button and the macOS password prompt.
// Debug builds never run it (enforced in Rust): they return the command.
//
// States: closed · loading · empty · plan · running · done · refused · error · debug.
// "cancelled" (password prompt dismissed) goes back to the plan with a banner.

const RESCUE_QUIT_COUNTDOWN_SECS = 60;
const RESCUE_RELEASES_URL = "https://github.com/jaylenjupp/it-hurts-when-ip/releases";
const RESCUE_DEFAULT_COMMAND =
  'sudo bash "/Applications/It Hurts When IP.app/Contents/Resources/ithurtswhenip-rescue.command"';
const RESCUE_DOWNLOAD_COMMAND = "sudo bash ~/Downloads/ithurtswhenip-rescue.command";

const rescue = {
  state: "closed",
  plan: null,
  picks: new Set(),
  restore: false,
  replanning: false,
  seq: 0,
  run: null,
  message: "",
  banner: "",
  canGoBack: false,
  focusKey: "",
  countdown: 0,
  timer: null,
  overlay: document.getElementById("rescue-modal-overlay"),
  modal: document.getElementById("rescue-modal"),
  title: document.getElementById("rescue-modal-title"),
  body: document.getElementById("rescue-modal-body"),
  buttons: document.getElementById("rescue-modal-buttons"),
};

function rescueEl(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

// Once stage B has started, the only ways out are "Finish and quit" and the countdown.
function rescueStageBPending() {
  return rescue.state === "done" && rescue.run && rescue.run.stage_b_started;
}

function rescueCanClose() {
  return rescue.state !== "closed" && rescue.state !== "running" && !rescueStageBPending();
}

function openRescue() {
  if (rescue.state !== "closed" || repair.state !== "closed" || diag.state !== "closed") return; // one flow at a time
  rescue.plan = null;
  rescue.picks = new Set();
  rescue.restore = false;
  rescue.run = null;
  rescue.message = "";
  rescue.focusKey = "";
  rescue.overlay.classList.remove("hidden");
  loadRescuePlan();
}

function closeRescue() {
  if (!rescueCanClose()) return;
  rescue.seq++; // drop any plan still on its way
  rescue.state = "closed";
  rescue.replanning = false;
  rescue.overlay.classList.add("hidden");
  rescue.modal.dataset.state = "closed";
  settingsRefocus("settings-uninstall");
}

// Fetches a fresh plan for the current choices. Stale replies are dropped.
async function loadRescuePlan(banner = "") {
  const mySeq = ++rescue.seq;
  rescue.banner = banner;
  if (rescue.state === "plan" && rescue.plan) {
    rescue.replanning = true;
  } else {
    rescue.state = "loading";
  }
  renderRescue();

  let plan;
  try {
    plan = await invoke("rescue_plan", {
      dhcp: [...rescue.picks],
      restoreJournal: rescue.restore,
    });
  } catch (error) {
    if (mySeq !== rescue.seq) return;
    rescue.replanning = false;
    rescue.state = "error";
    rescue.message = String(error);
    rescue.canGoBack = rescue.plan !== null;
    renderRescue();
    return;
  }
  if (mySeq !== rescue.seq) return;
  rescue.replanning = false;

  if (plan.refused) {
    rescue.state = "refused";
    rescue.message = plan.refused;
    renderRescue();
    return;
  }

  // Drop choices the fresh plan no longer offers, then plan again without them.
  const offered = new Set(plan.manual.map((m) => m.name));
  const stale = [...rescue.picks].filter((name) => !offered.has(name));
  const staleRestore = rescue.restore && !plan.journal_usable;
  rescue.plan = plan;
  if (stale.length || staleRestore) {
    stale.forEach((name) => rescue.picks.delete(name));
    if (staleRestore) rescue.restore = false;
    rescue.state = "plan";
    loadRescuePlan(banner);
    return;
  }

  rescue.state = plan.nothing_to_remove ? "empty" : "plan";
  renderRescue();
}

function rescueBackToPlan() {
  rescue.picks = new Set();
  rescue.restore = false;
  rescue.plan = null;
  rescue.focusKey = "cancel";
  document.activeElement?.blur();
  rescue.state = "loading";
  loadRescuePlan();
}

async function confirmRescue() {
  if (rescue.state !== "plan" || rescue.replanning) return;
  rescue.state = "running";
  rescue.banner = "";
  rescue.focusKey = "cancel"; // never land back on the red button after a cancel
  document.activeElement?.blur();
  renderRescue();

  let run;
  try {
    run = await invoke("rescue_uninstall", {
      dhcp: [...rescue.picks],
      restoreJournal: rescue.restore,
    });
  } catch (error) {
    rescue.state = "error";
    rescue.message = String(error);
    rescue.canGoBack = true;
    renderRescue();
    return;
  }

  rescue.run = run;
  switch (run.outcome) {
    case "cancelled":
      loadRescuePlan("You closed the password prompt. Nothing was changed.");
      return;
    case "not_executed":
      rescue.state = "debug";
      break;
    case "refused":
      rescue.state = "refused";
      rescue.message = run.messages.join(" ") || "The rescue script refused to run.";
      break;
    case "ok":
    case "partial":
      rescue.state = "done";
      break;
    default:
      rescue.state = "error";
      rescue.message = run.messages.join(" ") || "The uninstall could not run.";
      rescue.canGoBack = true;
  }
  renderRescue();
  if (rescueStageBPending()) startRescueCountdown();
}

// Stage B (removing the app bundle) only finishes once the app has quit, and
// gives up after 300 s; Rust also quits the app after 150 s as a backstop.
function startRescueCountdown() {
  clearInterval(rescue.timer);
  rescue.countdown = RESCUE_QUIT_COUNTDOWN_SECS;
  updateRescueCountdown();
  rescue.timer = setInterval(() => {
    rescue.countdown -= 1;
    updateRescueCountdown();
    if (rescue.countdown <= 0) {
      clearInterval(rescue.timer);
      rescueQuit();
    }
  }, 1000);
}

// Only the countdown text changes each second, so selected text stays selected.
function updateRescueCountdown() {
  const line = document.getElementById("rescue-countdown");
  if (line) {
    const n = Math.max(rescue.countdown, 0);
    line.textContent = `The app closes in ${n} s so the uninstall can finish.`;
  }
}

function rescueQuit() {
  invoke("rescue_quit").catch((e) => console.error("[rescue_quit failed]", e));
}

// --- Uninstall modal: text for plan records and result lines ---

const RESCUE_WRITE_KINDS = ["setdhcp", "setmanual", "unchanged", "skip"];

const RESCUE_GROUPS = [
  { title: "Stop", match: (r) => r.kind === "stop" || r.kind === "quit" },
  { title: "Back up first", match: (r) => r.kind === "backup" },
  {
    title: "Change network settings",
    match: (r) => r.kind === "network" && RESCUE_WRITE_KINDS.includes(r.sub_kind),
  },
  { title: "Remove", match: (r) => r.kind === "remove" },
  { title: "Forget installer receipts", match: (r) => r.kind === "forget" },
  {
    title: "Notes",
    match: (r) => r.kind === "note" && !(r.subject === "-" && r.detail === "nothing to remove"),
  },
  { title: "Found but not removed (reported only)", match: (r) => r.kind === "leftover" },
  { title: "Other lines from the script", match: (r) => r.kind === "other" },
];

// "<mode>/<ip>/<mask>/<router>" as the script prints it, in words.
function rescueViewText(view) {
  const [mode, ip, mask, router] = String(view).split("/");
  if (mode === "dhcp") return "DHCP";
  if (mode === "manual") {
    return `Manual ${ip} / ${mask}` + (router && router !== "-" ? `, router ${router}` : "");
  }
  if (mode === "missing") return "service no longer exists";
  return mode || "unknown";
}

function rescueRecordText(r) {
  switch (r.kind) {
    case "stop":
      return "Background helper " + r.subject;
    case "quit":
      return "Quit " + r.subject;
    case "backup":
      return r.sub_kind === "snapshot" ? `Network settings snapshot (${r.detail})` : r.subject;
    case "remove":
      return r.detail && r.detail !== "-" ? `${r.subject} (${r.detail})` : r.subject;
    case "network":
      if (r.sub_kind === "setdhcp") return `${r.subject}: switch to DHCP`;
      if (r.sub_kind === "setmanual") {
        return `${r.subject}: set back to ${r.detail.replace(/\s*source=\S+$/, "")}`;
      }
      return `${r.subject}: ${r.detail}`;
    case "note":
      return r.subject && r.subject !== "-" ? `${r.subject}: ${r.detail}` : r.detail;
    case "forget":
    case "leftover":
      return r.subject;
    default:
      return r.detail;
  }
}

const RESCUE_STEP_LABELS = {
  BACKUP: "Back up",
  NETWORK: "Network",
  STOP: "Stop",
  QUIT: "Quit",
  REMOVE: "Remove",
  FORGET: "Forget receipt",
};

function rescueStepText(s) {
  if (s.kind === "VERIFY") {
    return "Checked " + s.subject + (s.detail ? ` (${s.detail})` : "");
  }
  let text = `${RESCUE_STEP_LABELS[s.kind] || s.kind} ${s.subject}`;
  if (s.detail && s.detail !== "-") text += ` (${s.detail})`;
  if (s.reason) text += ` — ${s.reason}`;
  return text;
}

function rescueStepFailed(s) {
  return s.status === "failed" || s.status === "verify_failed";
}

function rescueConfirmLabel(plan) {
  const parts = [];
  const n = rescue.picks.size;
  if (n) parts.push(`switch ${n} service${n === 1 ? "" : "s"} to DHCP`);
  if (rescue.restore) {
    const m = plan.records.filter(
      (r) =>
        r.kind === "network" &&
        (r.sub_kind === "setdhcp" || r.sub_kind === "setmanual") &&
        r.detail.includes("source=journal")
    ).length;
    // Recorded settings that already match are not rewritten, so 0 adds nothing.
    if (m) parts.push(`restore ${m} service${m === 1 ? "" : "s"}`);
  }
  return "Uninstall" + (parts.length ? " and " + parts.join(" and ") : "");
}

// --- Uninstall modal: rendering ---

function rescueButton(label, className, onClick, key, disabled = false) {
  const b = rescueEl("button", className, label);
  b.type = "button";
  b.dataset.key = key;
  b.disabled = disabled;
  b.addEventListener("click", onClick);
  return b;
}

function rescueSelectable(text) {
  return rescueEl("div", "rescue-path", text);
}

function rescueList(items, classFor) {
  const ul = rescueEl("ul", "rescue-list");
  items.forEach((item) => {
    const li = rescueEl("li", classFor ? classFor(item) : "", item.text);
    ul.appendChild(li);
  });
  return ul;
}

function rescueFallback(command) {
  const box = rescueEl("div", "rescue-box");
  box.appendChild(rescueEl("div", "rescue-muted", "Prefer Terminal, or the app is not working? Open Terminal and run:"));
  box.appendChild(rescueSelectable(command || RESCUE_DEFAULT_COMMAND));
  const missing = rescueEl("div", "rescue-muted");
  missing.appendChild(document.createTextNode("If the script is missing, download ithurtswhenip-rescue.command from the "));
  const link = rescueEl("button", "rescue-link", "Releases page");
  link.type = "button";
  link.dataset.key = "releases";
  link.addEventListener("click", () => openUrl(RESCUE_RELEASES_URL));
  missing.appendChild(link);
  missing.appendChild(document.createTextNode(" and run:"));
  box.appendChild(missing);
  box.appendChild(rescueSelectable(RESCUE_DOWNLOAD_COMMAND));
  return box;
}

function rescueCheckbox(key, checked, disabled, title, detail, onChange) {
  const label = rescueEl("label", "rescue-check");
  const input = rescueEl("input");
  input.type = "checkbox";
  input.checked = checked;
  input.disabled = disabled;
  input.dataset.key = key;
  input.addEventListener("change", () => onChange(input.checked));
  const text = rescueEl("span");
  text.appendChild(rescueEl("span", "", title));
  if (detail) text.appendChild(rescueEl("span", "rescue-check-detail", " " + detail));
  label.appendChild(input);
  label.appendChild(text);
  return label;
}

function renderRescuePlanBody(body, plan) {
  const busy = rescue.replanning;
  body.appendChild(
    rescueEl(
      "p",
      "",
      "This removes the app, its background helper and its settings (and old IP Switcher files), and backs up everything it removes first."
    )
  );

  // Network choices.
  body.appendChild(rescueEl("div", "rescue-section-title", "Network settings"));
  const net = rescueEl("div", "rescue-box");
  if (plan.manual.length) {
    net.appendChild(rescueEl("div", "rescue-muted", "Services on Manual (static) settings. Tick one to switch it to DHCP:"));
    plan.manual.forEach((m) => {
      const detail = `${m.ip} / ${m.mask}` + (m.router && m.router !== "-" ? `, router ${m.router}` : "");
      net.appendChild(
        rescueCheckbox("dhcp:" + m.name, rescue.picks.has(m.name), busy, m.name, detail, (on) => {
          if (on) rescue.picks.add(m.name);
          else rescue.picks.delete(m.name);
          loadRescuePlan();
        })
      );
    });
  } else {
    net.appendChild(rescueEl("div", "rescue-muted", "No enabled service is on Manual (static) settings."));
  }
  if (plan.journal_usable) {
    const n = plan.journal.length;
    net.appendChild(
      rescueCheckbox(
        "restore",
        rescue.restore,
        busy,
        `Restore the settings recorded before the app first changed them (${n} service${n === 1 ? "" : "s"}, IPv4 only)`,
        "",
        (on) => {
          rescue.restore = on;
          loadRescuePlan();
        }
      )
    );
    net.appendChild(
      rescueList(
        plan.journal.map((j) => ({
          text: `${j.name}: recorded ${rescueViewText(j.original)}, now ${rescueViewText(j.current)}`,
        })),
        () => "rescue-muted"
      )
    );
  }
  net.appendChild(rescueEl("div", "", "Network settings are otherwise left exactly as they are."));
  body.appendChild(net);

  // The plan, grouped.
  body.appendChild(rescueEl("div", "rescue-section-title", busy ? "What will happen (updating…)" : "What will happen"));
  if (plan.backup_folder) {
    body.appendChild(rescueEl("div", "rescue-muted", "Backups go to:"));
    body.appendChild(rescueSelectable(plan.backup_folder));
  }
  RESCUE_GROUPS.forEach((group) => {
    const items = plan.records.filter(group.match);
    if (!items.length) return;
    body.appendChild(rescueEl("div", "rescue-group-title", group.title));
    body.appendChild(rescueList(items.map((r) => ({ text: rescueRecordText(r) }))));
  });
  plan.notices.forEach((n) => body.appendChild(rescueEl("div", "rescue-muted", "Note: " + n)));

  body.appendChild(rescueFallback(plan.terminal_command));
}

function renderRescueResultBody(body, run) {
  const pending = rescueStageBPending();
  if (run.backup_folder) {
    body.appendChild(rescueEl("div", "rescue-muted", "Backup folder:"));
    body.appendChild(rescueSelectable(run.backup_folder));
  }
  if (run.log_path) {
    body.appendChild(rescueEl("div", "rescue-muted", "Full result:"));
    body.appendChild(rescueSelectable(run.log_path));
  }
  if (pending) {
    body.appendChild(rescueEl("div", "rescue-countdown", ""));
    body.lastChild.id = "rescue-countdown";
    body.appendChild(
      rescueEl(
        "div",
        "",
        "After the app closes, the rescue script removes the app and your settings in the background and adds its result to the log above."
      )
    );
  }
  run.messages.forEach((m) => body.appendChild(rescueEl("div", "rescue-error", m)));

  const failed = run.steps.filter(rescueStepFailed);
  const rest = run.steps.filter((s) => !rescueStepFailed(s));
  if (failed.length) {
    body.appendChild(rescueEl("div", "rescue-section-title", `Failed (${failed.length})`));
    body.appendChild(rescueList(failed.map((s) => ({ text: rescueStepText(s) })), () => "failed"));
  }
  if (rest.length) {
    body.appendChild(rescueEl("div", "rescue-section-title", "Done"));
    body.appendChild(rescueList(rest.map((s) => ({ text: rescueStepText(s) })), () => "ok"));
  }
}

function renderRescue() {
  const { body, buttons } = rescue;
  // Remember which control had focus, so a re-plan (which rebuilds the DOM and
  // briefly disables the checkboxes) gives it back afterwards.
  const active = document.activeElement;
  if (active && rescue.modal.contains(active) && active.dataset.key) {
    rescue.focusKey = active.dataset.key;
  }
  rescue.modal.dataset.state = rescue.state;
  body.replaceChildren();
  buttons.replaceChildren();
  rescue.title.textContent = "Uninstall It Hurts When IP";

  if (rescue.banner && (rescue.state === "plan" || rescue.state === "loading")) {
    body.appendChild(rescueEl("div", "rescue-banner", rescue.banner));
  }

  const cancel = (label = "Cancel") => rescueButton(label, "", closeRescue, "cancel");
  const back = () => rescueButton("Back to plan", "", rescueBackToPlan, "back");
  let defaultKey = "cancel";

  switch (rescue.state) {
    case "loading":
      body.appendChild(rescueEl("p", "rescue-muted", "Checking what is installed…"));
      buttons.appendChild(cancel());
      break;

    case "empty":
      body.appendChild(rescueEl("p", "", "It Hurts When IP is not installed on this Mac: there is nothing to remove."));
      buttons.appendChild(cancel("Close"));
      break;

    case "plan":
      renderRescuePlanBody(body, rescue.plan);
      buttons.appendChild(cancel());
      buttons.appendChild(
        rescueButton(rescueConfirmLabel(rescue.plan), "rescue-danger", confirmRescue, "confirm", rescue.replanning)
      );
      break;

    case "running":
      body.appendChild(rescueEl("p", "", "Waiting for your administrator password. macOS asks for it in a separate window."));
      body.appendChild(rescueEl("p", "rescue-muted", "Nothing changes until you enter it. Keep this window open."));
      buttons.appendChild(rescueButton("Uninstalling…", "", () => {}, "running", true));
      defaultKey = "";
      break;

    case "done": {
      const run = rescue.run;
      const failed = run.outcome === "partial";
      rescue.title.textContent = failed ? "Uninstall finished with problems" : "Uninstall finished";
      renderRescueResultBody(body, run);
      if (rescueStageBPending()) {
        buttons.appendChild(rescueButton("Finish and quit", "rescue-primary", rescueQuit, "quit"));
        defaultKey = "quit";
      } else {
        buttons.appendChild(cancel("Close"));
      }
      break;
    }

    case "refused":
      rescue.title.textContent = "Uninstall did not run";
      body.appendChild(rescueEl("p", "rescue-error", rescue.message));
      body.appendChild(rescueEl("p", "", "Nothing was changed."));
      buttons.appendChild(cancel("Close"));
      buttons.appendChild(back());
      break;

    case "error":
      rescue.title.textContent = "Uninstall could not run";
      body.appendChild(rescueEl("p", "rescue-error", rescue.message));
      body.appendChild(rescueFallback(rescue.plan ? rescue.plan.terminal_command : ""));
      buttons.appendChild(cancel("Close"));
      if (rescue.canGoBack) buttons.appendChild(back());
      break;

    case "debug":
      rescue.title.textContent = "Debug build: not executed";
      body.appendChild(rescueEl("p", "", "Debug builds never run the uninstall. A release build would run exactly this command (one argument per block):"));
      rescue.run.argv.forEach((arg) => body.appendChild(rescueSelectable(arg)));
      buttons.appendChild(cancel("Close"));
      buttons.appendChild(back());
      break;
  }

  updateRescueCountdown();
  const again =
    rescue.focusKey && rescue.modal.querySelector(`[data-key="${CSS.escape(rescue.focusKey)}"]`);
  if (again && again.disabled) return; // re-planning: focus comes back when it is enabled again
  const target = again || (defaultKey && buttons.querySelector(`[data-key="${defaultKey}"]`));
  if (target) target.focus();
}

listen("rescue-open", () => openRescue()).catch((e) => console.error("[rescue-open listen failed]", e));
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || rescue.state === "closed") return;
  e.preventDefault();
  if (rescueCanClose()) closeRescue();
});

// --- Repair network modal (rescue script --repair) ---
//
// Opened from the app menu ("Repair Network…") and Settings → Maintenance;
// never while the uninstall modal is open (and the other way
// round). Flow: choose a tier -> the script's read-only plan -> confirm (a
// full reset needs RESET typed) -> macOS password prompt -> result. Debug
// builds never run it (enforced in Rust): they return the command. The app
// never restarts the Mac: it only says when a restart is needed.
//
// States: closed · choose · loading · pick · plan · running · done · refused · error · debug.
// A dismissed password prompt goes back to the plan with a calm banner.

const REPAIR_RESET_WORD = "RESET";

const REPAIR_TIERS = [
  {
    tier: "soft",
    title: "Soft refresh",
    text:
      "Clears the Mac's DNS cache and asks adapters that get their address automatically (DHCP) for a fresh one. Manual (static) settings are not touched. Safe to try first.",
  },
  {
    tier: "service",
    title: "Reset one service",
    text:
      "Pick one adapter. Its IP settings go back to automatic (DHCP), its DNS servers and search domains are cleared, and it is turned off and on again. A static IP on it is removed. Its current settings are backed up first.",
  },
  {
    tier: "full",
    title: "Full network reset",
    text:
      "Last resort: use only when nothing else works. Moves the Mac's network settings files to a backup so macOS rebuilds them after you restart. Every adapter, static IP, Location and service order starts from scratch, and Wi-Fi networks must be rejoined.",
  },
];

const repair = {
  state: "closed",
  tier: "",
  service: "",
  services: null, // the soft plan's service list, for the "Reset one service" picker
  plan: null,
  typed: "",
  run: null,
  message: "",
  banner: "",
  canGoBack: false,
  focusKey: "",
  seq: 0,
  overlay: document.getElementById("repair-modal-overlay"),
  modal: document.getElementById("repair-modal"),
  title: document.getElementById("repair-modal-title"),
  body: document.getElementById("repair-modal-body"),
  buttons: document.getElementById("repair-modal-buttons"),
};

function repairCanClose() {
  return repair.state !== "closed" && repair.state !== "running";
}

function openRepair() {
  if (repair.state !== "closed" || rescue.state !== "closed" || diag.state !== "closed") return;
  repairReset();
  repair.overlay.classList.remove("hidden");
  repair.state = "choose";
  renderRepair();
}

function repairReset() {
  repair.tier = "";
  repair.service = "";
  repair.services = null;
  repair.plan = null;
  repair.typed = "";
  repair.run = null;
  repair.message = "";
  repair.banner = "";
  repair.canGoBack = false;
  repair.focusKey = "";
}

function closeRepair() {
  if (!repairCanClose()) return;
  repair.seq++; // drop any plan still on its way
  repair.typed = "";
  repair.state = "closed";
  repair.overlay.classList.add("hidden");
  repair.modal.dataset.state = "closed";
  settingsRefocus("settings-repair");
}

// Back to the three tiers; forgets the plan and the typed word.
function repairBackToChoose() {
  repair.seq++;
  const tier = repair.tier;
  repairReset();
  repair.focusKey = "tier:" + tier;
  document.activeElement?.blur();
  repair.state = "choose";
  renderRepair();
}

async function chooseRepairTier(tier) {
  repair.tier = tier;
  repair.service = "";
  repair.plan = null;
  repair.typed = "";
  repair.banner = "";
  repair.focusKey = "";
  if (tier === "service") {
    await loadRepairServices();
  } else {
    await loadRepairPlan();
  }
}

// The services the script itself reports (its read-only soft plan), so the
// picker only offers names the script accepts. Preselects the main window's
// interface when it can be reset.
async function loadRepairServices() {
  const mySeq = ++repair.seq;
  repair.state = "loading";
  renderRepair();
  let plan;
  try {
    plan = await invoke("repair_plan", { tier: "soft", service: null });
  } catch (error) {
    if (mySeq !== repair.seq) return;
    repair.state = "error";
    repair.message = String(error);
    repair.canGoBack = true;
    renderRepair();
    return;
  }
  if (mySeq !== repair.seq) return;
  if (plan.refused) {
    repair.state = "refused";
    repair.message = plan.refused;
    renderRepair();
    return;
  }
  repair.services = plan.services;
  const current = document.getElementById("interface-select")?.value || "";
  const offered = plan.services.filter((s) => s.resettable).map((s) => s.name);
  if (offered.includes(current)) {
    repair.service = current;
    loadRepairPlan();
    return;
  }
  repair.state = "pick";
  repair.focusKey = "service-pick";
  renderRepair();
}

function pickRepairService(name) {
  repair.service = name;
  repair.plan = null;
  repair.banner = "";
  repair.focusKey = "service-pick";
  if (name) loadRepairPlan();
  else {
    repair.state = "pick";
    renderRepair();
  }
}

// Fetches a fresh plan for the chosen tier (and service). Stale replies are dropped.
async function loadRepairPlan(banner = "") {
  const mySeq = ++repair.seq;
  repair.banner = banner;
  repair.state = "loading";
  renderRepair();
  let plan;
  try {
    plan = await invoke("repair_plan", {
      tier: repair.tier,
      service: repair.tier === "service" ? repair.service : null,
    });
  } catch (error) {
    if (mySeq !== repair.seq) return;
    repair.state = "error";
    repair.message = String(error);
    repair.canGoBack = true;
    renderRepair();
    return;
  }
  if (mySeq !== repair.seq) return;
  repair.plan = plan;
  if (plan.refused) {
    repair.state = "refused";
    repair.message = plan.refused;
  } else {
    repair.state = "plan";
  }
  renderRepair();
}

function repairConfirmReady() {
  return repair.tier !== "full" || repair.typed === REPAIR_RESET_WORD;
}

async function confirmRepair() {
  if (repair.state !== "plan" || !repairConfirmReady()) return;
  const confirm = repair.tier === "full" ? repair.typed : "";
  repair.state = "running";
  repair.banner = "";
  repair.focusKey = "cancel"; // never land back on the confirm button after a cancel
  document.activeElement?.blur();
  renderRepair();

  let run;
  try {
    run = await invoke("repair_run", {
      tier: repair.tier,
      service: repair.tier === "service" ? repair.service : null,
      confirm,
    });
  } catch (error) {
    repair.typed = "";
    repair.state = "error";
    repair.message = String(error);
    repair.canGoBack = true;
    renderRepair();
    return;
  }

  repair.typed = "";
  repair.run = run;
  switch (run.outcome) {
    case "cancelled":
      loadRepairPlan("Cancelled, nothing changed.");
      return;
    case "not_executed":
      repair.state = "debug";
      break;
    case "refused":
      repair.state = "refused";
      repair.message = run.messages.join(" ") || "The rescue script refused to run.";
      break;
    case "ok":
    case "partial":
      repair.state = "done";
      break;
    default:
      repair.state = "error";
      repair.message = run.messages.join(" ") || "The repair could not run.";
      repair.canGoBack = true;
  }
  renderRepair();
}

// --- Repair modal: text for plan records and result lines ---

function repairFileName(path) {
  return String(path).split("/").pop();
}

// "now=<m>/<ip>/<mask>/<router> …" -> the view part.
function repairNowView(detail) {
  const m = /(?:^|\s)now=(\S+)/.exec(detail || "");
  return m ? m[1] : "";
}

function repairAddressText(view) {
  const [mode, ip, mask, router] = String(view).split("/");
  const where = ip && ip !== "-" ? `${ip} / ${mask}` + (router && router !== "-" ? `, router ${router}` : "") : "no address";
  if (mode === "dhcp") return `automatic (DHCP), ${where}`;
  if (mode === "manual") return `manual (static) ${where}`;
  return mode || "unknown";
}

function repairWas(detail) {
  const v = String(detail || "").replace(/^was=/, "");
  return v === "-" || v === "" ? "none" : v.split(",").join(", ");
}

// Plan records and OK/FAILED lines (same TYPE KIND SUBJECT DETAIL fields).
function repairActionText(kind, sub, subject, detail) {
  const k = String(kind).toLowerCase();
  if (k === "repair") {
    switch (sub) {
      case "flushcache":
        return `Clear the DNS cache (${detail})`;
      case "hup":
        return `Restart the DNS responder (${detail})`;
      case "renew": {
        const dev = /device=(\S+)/.exec(detail || "");
        return `${subject}: ask for a fresh address (now ${repairAddressText(repairNowView(detail))}${dev ? ", device " + dev[1] : ""})`;
      }
      case "leave":
        return `${subject}: ${detail}`;
      case "setdhcp":
        return `${subject}: switch to automatic (DHCP) (was ${repairAddressText(String(detail).replace(/^was=/, ""))})`;
      case "cleardns":
        return `${subject}: clear DNS servers (was ${repairWas(detail)})`;
      case "clearsearch":
        return `${subject}: clear search domains (was ${repairWas(detail)})`;
      case "disable":
        return `${subject}: turn off`;
      case "enable":
        return `${subject}: turn on again`;
      default:
        return `${subject}: ${detail}`;
    }
  }
  if (k === "backup") {
    if (sub === "snapshot") return `Save a snapshot of the settings (${detail})`;
    if (sub === "file") return `Back up ${subject}`;
    if (sub === "manifest") return `Write the backup checksums (${detail})`;
    if (sub === "check") return `Check the backup copies (${detail})`;
    return `Back up ${subject}`;
  }
  if (k === "reset") {
    if (sub === "move") return `Move ${repairFileName(subject)} to the backup`;
    return `${repairFileName(subject)}: ${detail}`;
  }
  return [subject, detail].filter((x) => x && x !== "-").join(": ");
}

function repairStepText(s) {
  if (s.kind === "VERIFY") {
    const sep = s.subject.indexOf(":");
    const what = sep > 0 ? s.subject.slice(0, sep) : "";
    const name = sep > 0 ? s.subject.slice(sep + 1) : s.subject;
    if (what === "repair") {
      const view = repairNowView(s.detail);
      if (s.status === "verify_ok") return `${name}: working, ${repairAddressText(view)}`;
      if (s.status === "verify_warn") {
        const linkLocal = /link-local/.test(s.detail) ? " (only a self-assigned 169.254 address)" : "";
        return `${name}: No address yet${linkLocal} — check cable/switch/DHCP server`;
      }
      return `${name}: not as expected (${s.detail})`;
    }
    if (what === "reset") {
      if (s.status === "verify_ok") return `${repairFileName(name)}: moved, backup copy checked`;
      return `${repairFileName(name)}: ${s.detail || "check failed"}`;
    }
    return "Checked " + s.subject + (s.detail ? ` (${s.detail})` : "");
  }
  let text = repairActionText(s.kind, s.sub_kind, s.subject, s.detail);
  if (s.reason) text += ` — failed: ${s.reason}`;
  return text;
}

// --- Repair modal: rendering ---

function renderRepairChoose(body) {
  body.appendChild(rescueEl("p", "", "Choose how much to repair. You see the exact plan before anything changes."));
  REPAIR_TIERS.forEach((t) => {
    const card = rescueEl("button", "repair-tier" + (t.tier === "full" ? " repair-tier-danger" : ""));
    card.type = "button";
    card.dataset.key = "tier:" + t.tier;
    card.appendChild(rescueEl("span", "repair-tier-title", t.title));
    card.appendChild(rescueEl("span", "repair-tier-text", t.text));
    card.addEventListener("click", () => chooseRepairTier(t.tier));
    body.appendChild(card);
  });
}

function renderRepairPicker(body, disabled) {
  const box = rescueEl("div", "rescue-box");
  const label = rescueEl("label", "repair-pick-label", "Service to reset");
  label.htmlFor = "repair-service-pick";
  box.appendChild(label);
  const select = rescueEl("select", "repair-pick");
  select.id = "repair-service-pick";
  select.dataset.key = "service-pick";
  select.disabled = disabled;
  const none = rescueEl("option", "", "Choose a service…");
  none.value = "";
  select.appendChild(none);
  (repair.services || []).forEach((s) => {
    const o = rescueEl("option", "", s.resettable ? s.name : `${s.name} — ${s.detail}`);
    o.value = s.name;
    o.disabled = !s.resettable;
    select.appendChild(o);
  });
  select.value = repair.service;
  select.addEventListener("change", () => pickRepairService(select.value));
  box.appendChild(select);
  if (!(repair.services || []).some((s) => s.resettable)) {
    box.appendChild(rescueEl("div", "rescue-muted", "No enabled service can be reset."));
  }
  body.appendChild(box);
}

function repairGroup(body, title, items, cls) {
  if (!items.length) return;
  body.appendChild(rescueEl("div", "rescue-group-title", title));
  body.appendChild(rescueList(items.map((text) => ({ text })), cls ? () => cls : undefined));
}

function renderRepairPlanBody(body, plan) {
  const rec = (kind, subs) => plan.records.filter((r) => r.kind === kind && (!subs || subs.includes(r.sub_kind)));
  const text = (r) => repairActionText(r.kind, r.sub_kind, r.subject, r.detail);

  if (plan.tier === "soft") {
    body.appendChild(rescueEl("p", "", REPAIR_TIERS[0].text));
    body.appendChild(rescueEl("div", "rescue-section-title", "What will happen"));
    repairGroup(body, "Back up first", rec("backup").map(text));
    repairGroup(body, "Refresh", rec("repair", ["flushcache", "hup", "renew"]).map(text));
    repairGroup(body, "Left alone", rec("repair", ["leave"]).map(text), "rescue-muted");
  } else if (plan.tier === "service") {
    renderRepairPicker(body, false);
    const c = plan.current;
    if (c) {
      body.appendChild(rescueEl("div", "rescue-section-title", `What gets cleared on “${plan.service}”`));
      const box = rescueEl("div", "rescue-box");
      box.appendChild(rescueEl("div", "", "IP settings: " + repairAddressText(c.now)));
      box.appendChild(rescueEl("div", "", "DNS servers: " + (c.dns.length ? c.dns.join(", ") : "none")));
      box.appendChild(rescueEl("div", "", "Search domains: " + (c.search.length ? c.search.join(", ") : "none")));
      box.appendChild(
        rescueEl("div", "rescue-muted", "Afterwards it gets its address automatically (DHCP) with no DNS servers or search domains of its own. It drops off the network for a few seconds while it is turned off and on.")
      );
      body.appendChild(box);
    }
    body.appendChild(rescueEl("div", "rescue-section-title", "What will happen"));
    repairGroup(body, "Back up first", rec("backup").map(text));
    repairGroup(body, "Reset", rec("repair", ["setdhcp", "cleardns", "clearsearch", "disable", "enable"]).map(text));
  } else {
    const warn = rescueEl("div", "repair-consequences");
    warn.appendChild(rescueEl("div", "repair-consequences-title", "Before you go on"));
    warn.appendChild(
      rescueList(
        [
          { text: "After you restart, macOS rebuilds every network service from scratch: static IPs, Locations and the service order are gone." },
          { text: "Wi-Fi networks may need to be rejoined." },
          { text: "Nothing changes on the running network until you restart the Mac yourself. The app never restarts it." },
          { text: "Use this only when nothing else works." },
        ],
        () => ""
      )
    );
    body.appendChild(warn);
    body.appendChild(rescueEl("div", "rescue-section-title", "What will happen"));
    repairGroup(body, "Back up first", rec("backup").map(text));
    repairGroup(body, "Move to the backup", rec("reset", ["move"]).map((r) => r.subject));
    repairGroup(body, "Not moved", rec("reset", ["absent", "skip"]).map(text), "rescue-muted");
  }

  // A full reset's own "reset" notes say what the consequences box above says.
  const notes = plan.records
    .filter((r) => r.kind === "note" && !(plan.tier === "full" && r.subject === "reset"))
    .map((r) => r.detail);
  repairGroup(body, "Notes", notes, "rescue-muted");
  repairGroup(body, "Other lines from the script", rec("other").map((r) => r.detail), "rescue-muted");
  if (plan.backup_folder) {
    body.appendChild(rescueEl("div", "rescue-muted", "Backups go to:"));
    body.appendChild(rescueSelectable(plan.backup_folder));
  }
  plan.notices.forEach((n) => body.appendChild(rescueEl("div", "rescue-muted", "Note: " + n)));

  if (plan.tier === "full") {
    const box = rescueEl("div", "rescue-box");
    const label = rescueEl("label", "repair-pick-label", `Type ${REPAIR_RESET_WORD} to confirm`);
    label.htmlFor = "repair-typed";
    box.appendChild(label);
    const input = rescueEl("input", "repair-typed");
    input.type = "text";
    input.id = "repair-typed";
    input.dataset.key = "typed";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.setAttribute("autocapitalize", "off");
    input.setAttribute("autocorrect", "off");
    input.value = repair.typed;
    input.addEventListener("input", () => {
      repair.typed = input.value;
      const go = repair.buttons.querySelector('[data-key="confirm"]');
      if (go) go.disabled = !repairConfirmReady();
    });
    box.appendChild(input);
    body.appendChild(box);
  }
  body.appendChild(rescueFallback(plan.terminal_command));
}

function renderRepairResultBody(body, run) {
  if (run.restart_needed) {
    const panel = rescueEl("div", "repair-restart");
    panel.appendChild(rescueEl("div", "repair-restart-title", "Restart needed"));
    panel.appendChild(
      rescueEl("div", "", "To finish the reset, restart this Mac when it suits you: Apple menu  → Restart…")
    );
    panel.appendChild(
      rescueEl("div", "", "Until then the network keeps working with its current settings. The app does not restart the Mac.")
    );
    body.appendChild(panel);
  }
  run.messages.forEach((m) => body.appendChild(rescueEl("div", "rescue-error", m)));

  const failed = run.steps.filter(rescueStepFailed);
  const warned = run.steps.filter((s) => s.status === "verify_warn");
  const rest = run.steps.filter((s) => !rescueStepFailed(s) && s.status !== "verify_warn");
  if (failed.length) {
    body.appendChild(rescueEl("div", "rescue-section-title", `Failed (${failed.length})`));
    body.appendChild(rescueList(failed.map((s) => ({ text: repairStepText(s) })), () => "failed"));
  }
  if (warned.length) {
    body.appendChild(rescueEl("div", "rescue-section-title", `Needs a look (${warned.length})`));
    body.appendChild(rescueList(warned.map((s) => ({ text: repairStepText(s) })), () => "warn"));
  }
  if (rest.length) {
    body.appendChild(rescueEl("div", "rescue-section-title", "Done"));
    body.appendChild(rescueList(rest.map((s) => ({ text: repairStepText(s) })), () => "ok"));
  }
  if (run.backup_folder) {
    body.appendChild(rescueEl("div", "rescue-muted", "Backup folder:"));
    body.appendChild(rescueSelectable(run.backup_folder));
  }
  if (run.log_path) {
    body.appendChild(rescueEl("div", "rescue-muted", "Full result:"));
    body.appendChild(rescueSelectable(run.log_path));
  }
  if (run.outcome === "partial") body.appendChild(rescueFallback(run.terminal_command));
}

function repairConfirmLabel() {
  if (repair.tier === "soft") return "Refresh network";
  if (repair.tier === "service") return `Reset “${repair.service}”`;
  return "Reset network settings";
}

function renderRepair() {
  const { body, buttons } = repair;
  const active = document.activeElement;
  if (active && repair.modal.contains(active) && active.dataset.key) {
    repair.focusKey = active.dataset.key;
  }
  repair.modal.dataset.state = repair.state;
  body.replaceChildren();
  buttons.replaceChildren();
  const tierTitle = (REPAIR_TIERS.find((t) => t.tier === repair.tier) || {}).title;
  repair.title.textContent = tierTitle ? `Repair network: ${tierTitle}` : "Repair network";

  if (repair.banner && (repair.state === "plan" || repair.state === "loading")) {
    body.appendChild(rescueEl("div", "rescue-banner", repair.banner));
  }

  const cancel = (label = "Cancel") => rescueButton(label, "", closeRepair, "cancel");
  const back = () => rescueButton("Back", "", repairBackToChoose, "back");
  let defaultKey = "cancel";

  switch (repair.state) {
    case "choose":
      renderRepairChoose(body);
      buttons.appendChild(cancel());
      defaultKey = "tier:soft";
      break;

    case "loading":
      if (repair.tier === "service" && repair.services) renderRepairPicker(body, true);
      body.appendChild(rescueEl("p", "rescue-muted", "Checking the network settings…"));
      buttons.appendChild(cancel());
      buttons.appendChild(rescueButton("Back", "", repairBackToChoose, "back", true));
      break;

    case "pick":
      body.appendChild(rescueEl("p", "", REPAIR_TIERS[1].text));
      renderRepairPicker(body, false);
      buttons.appendChild(cancel());
      buttons.appendChild(back());
      defaultKey = "service-pick";
      break;

    case "plan":
      renderRepairPlanBody(body, repair.plan);
      buttons.appendChild(cancel());
      buttons.appendChild(back());
      buttons.appendChild(
        rescueButton(
          repairConfirmLabel(),
          repair.tier === "soft" ? "rescue-primary" : "rescue-danger",
          confirmRepair,
          "confirm",
          !repairConfirmReady()
        )
      );
      defaultKey = repair.tier === "full" ? "typed" : "cancel";
      break;

    case "running":
      body.appendChild(rescueEl("p", "", "Waiting for your administrator password. macOS asks for it in a separate window."));
      body.appendChild(
        rescueEl("p", "rescue-muted", "Nothing changes until you enter it. After that the repair runs and checks the result, which can take up to about 15 seconds. Keep this window open.")
      );
      buttons.appendChild(rescueButton("Repairing…", "", () => {}, "running", true));
      defaultKey = "";
      break;

    case "done": {
      const partial = repair.run.outcome === "partial";
      repair.title.textContent = partial ? "Repair finished with problems" : "Repair finished";
      renderRepairResultBody(body, repair.run);
      buttons.appendChild(cancel("Close"));
      defaultKey = "cancel";
      break;
    }

    case "refused":
      repair.title.textContent = "Repair did not run";
      body.appendChild(rescueEl("p", "rescue-error", repair.message));
      body.appendChild(rescueEl("p", "", "Nothing was changed."));
      buttons.appendChild(cancel("Close"));
      buttons.appendChild(back());
      break;

    case "error":
      repair.title.textContent = "Repair could not run";
      body.appendChild(rescueEl("p", "rescue-error", repair.message));
      body.appendChild(rescueFallback(repair.plan ? repair.plan.terminal_command : ""));
      buttons.appendChild(cancel("Close"));
      if (repair.canGoBack) buttons.appendChild(back());
      break;

    case "debug":
      repair.title.textContent = "Debug build: not executed";
      body.appendChild(rescueEl("p", "", "Debug builds never run the repair. A release build would run exactly this command (one argument per block):"));
      repair.run.argv.forEach((arg) => body.appendChild(rescueSelectable(arg)));
      buttons.appendChild(cancel("Close"));
      buttons.appendChild(back());
      break;
  }

  const again =
    repair.focusKey && repair.modal.querySelector(`[data-key="${CSS.escape(repair.focusKey)}"]`);
  const target =
    (again && !again.disabled && again) ||
    (defaultKey && repair.modal.querySelector(`[data-key="${CSS.escape(defaultKey)}"]`));
  if (target && !target.disabled) target.focus();
}

listen("repair-open", () => openRepair()).catch((e) => console.error("[repair-open listen failed]", e));
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || repair.state === "closed") return;
  e.preventDefault();
  if (repairCanClose()) closeRepair();
});

// --- Copy diagnostics modal ---
//
// Opened from Settings → Maintenance → "Copy diagnostics…". Shows the
// read-only report from get_diagnostics (Rust builds the text) and copies it
// with navigator.clipboard. Never open together with the uninstall or repair
// modal. Nothing here changes anything.
//
// States: closed · loading · shown · error.

const DIAG_COPY_FAILED = "Couldn't copy automatically — the text is selected, press ⌘C";
const DIAG_COPIED_MS = 2000;

const diag = {
  state: "closed",
  text: "",
  message: "",
  seq: 0,
  copiedTimer: 0,
  textEl: null,
  copyButton: null,
  copyNote: null,
  overlay: document.getElementById("diag-modal-overlay"),
  modal: document.getElementById("diag-modal"),
  title: document.getElementById("diag-modal-title"),
  body: document.getElementById("diag-modal-body"),
  buttons: document.getElementById("diag-modal-buttons"),
};

function openDiagnostics() {
  if (diag.state !== "closed" || rescue.state !== "closed" || repair.state !== "closed") return;
  diag.overlay.classList.remove("hidden");
  loadDiagnostics();
}

function closeDiagnostics() {
  if (diag.state === "closed") return;
  diag.seq++; // drop a report still on its way
  clearTimeout(diag.copiedTimer);
  diag.state = "closed";
  diag.text = "";
  diag.textEl = diag.copyButton = diag.copyNote = null;
  diag.overlay.classList.add("hidden");
  diag.modal.dataset.state = "closed";
  diag.body.replaceChildren();
  diag.buttons.replaceChildren();
  settingsRefocus("settings-copy-diagnostics");
}

// Also the Retry after an error. Passes the main window's interface.
async function loadDiagnostics() {
  const mySeq = ++diag.seq;
  diag.state = "loading";
  diag.text = "";
  diag.message = "";
  renderDiagnostics();
  const selected = interfaceSelect.value || null;
  try {
    const result = await invoke("get_diagnostics", { selectedService: selected });
    if (mySeq !== diag.seq) return;
    diag.text = result && typeof result.text === "string" ? result.text : "";
    if (!diag.text) throw new Error("the report came back empty");
    diag.state = "shown";
  } catch (error) {
    if (mySeq !== diag.seq) return;
    diag.state = "error";
    diag.message = String(error);
  }
  renderDiagnostics();
}

function selectDiagnosticsText() {
  if (!diag.textEl) return;
  diag.textEl.focus();
  diag.textEl.select();
}

// Updates the Copy button and note in place, so the text keeps its scroll
// position and selection.
async function copyDiagnostics() {
  if (diag.state !== "shown" || !diag.copyButton) return;
  clearTimeout(diag.copiedTimer);
  diag.copyNote.textContent = "";
  diag.copyNote.classList.add("hidden");
  try {
    await navigator.clipboard.writeText(diag.text);
  } catch (error) {
    console.warn("[clipboard write failed]", error);
    if (diag.state !== "shown" || !diag.copyNote) return;
    diag.copyButton.textContent = "Copy";
    diag.copyNote.textContent = DIAG_COPY_FAILED;
    diag.copyNote.classList.remove("hidden");
    selectDiagnosticsText();
    return;
  }
  if (diag.state !== "shown" || !diag.copyButton) return;
  diag.copyButton.textContent = "Copied";
  diag.copiedTimer = setTimeout(() => {
    if (diag.copyButton) diag.copyButton.textContent = "Copy";
  }, DIAG_COPIED_MS);
}

function renderDiagnostics() {
  const { body, buttons } = diag;
  diag.modal.dataset.state = diag.state;
  body.replaceChildren();
  buttons.replaceChildren();
  diag.textEl = diag.copyButton = diag.copyNote = null;
  diag.title.textContent = "Diagnostics";
  const close = rescueButton("Close", "", closeDiagnostics, "close");
  let focus = close;

  switch (diag.state) {
    case "loading":
      body.appendChild(rescueEl("p", "rescue-muted", "Collecting diagnostics…"));
      buttons.appendChild(close);
      buttons.appendChild(rescueButton("Copy", "rescue-primary", () => {}, "copy", true));
      break;

    case "shown": {
      body.appendChild(rescueEl("p", "", "Includes your current network addresses. Check before sharing."));
      const text = document.createElement("textarea");
      text.className = "diag-text";
      text.readOnly = true;
      text.spellcheck = false;
      text.value = diag.text;
      text.setAttribute("aria-label", "Diagnostics text");
      body.appendChild(text);
      const note = rescueEl("p", "diag-copy-note hidden", "");
      note.setAttribute("role", "status");
      body.appendChild(note);
      const copy = rescueButton("Copy", "rescue-primary", copyDiagnostics, "copy");
      buttons.appendChild(close);
      buttons.appendChild(copy);
      diag.textEl = text;
      diag.copyButton = copy;
      diag.copyNote = note;
      focus = copy;
      break;
    }

    case "error": {
      diag.title.textContent = "Diagnostics could not be collected";
      body.appendChild(rescueEl("p", "rescue-error", diag.message));
      body.appendChild(rescueEl("p", "", "Nothing was changed. Try again, or close and report the message above."));
      const retry = rescueButton("Retry", "rescue-primary", loadDiagnostics, "retry");
      buttons.appendChild(close);
      buttons.appendChild(retry);
      focus = retry;
      break;
    }
  }
  focus.focus();
}

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || diag.state === "closed") return;
  e.preventDefault();
  closeDiagnostics();
});

// --- Settings panel ---
//
// In-window full panel, opened from the gear (#open-settings), the app menu
// ("Settings…", ⌘, — the menu accelerator, so no JS shortcut here) and closed
// with ✕ or Esc. Never opens while the uninstall or repair modal is open; those
// modals open on top of it and hand focus back to their Settings button when
// they close. Every open re-reads the settings (that is also the retry after
// a load error). A saved setting is just the toggle position: no toasts.

// Current settings for other parts of the UI (4.4 reads advanced_mode).
// `loaded` is false until a get_settings call has succeeded.
const settingsState = {
  loaded: false,
  advanced_mode: false,
  check_updates_on_launch: true,
};

const settingsListeners = [];

// Calls fn(settingsState) after every successful load or save. The same
// change is also sent as a window "ihw-settings-change" event.
function onSettingsChange(fn) {
  settingsListeners.push(fn);
}

function notifySettingsChange() {
  for (const fn of settingsListeners) {
    try {
      fn(settingsState);
    } catch (e) {
      console.error("[settings listener failed]", e);
    }
  }
  window.dispatchEvent(new CustomEvent("ihw-settings-change", { detail: { ...settingsState } }));
}

const settingsUi = {
  open: false,
  seq: 0,
  container: document.querySelector(".container"),
  gear: document.getElementById("open-settings"),
  overlay: document.getElementById("settings-overlay"),
  panel: document.getElementById("settings-panel"),
  status: document.getElementById("settings-status"),
  checkUpdates: document.getElementById("settings-check-updates"),
  checkUpdatesError: document.getElementById("settings-check-updates-error"),
  buildVersion: document.getElementById("settings-build-version"),
  buildStamp: document.getElementById("settings-build-stamp"),
  advanced: document.getElementById("settings-advanced"),
  advancedError: document.getElementById("settings-advanced-error"),
  advancedDetail: document.getElementById("settings-advanced-detail"),
  confirm: document.getElementById("settings-advanced-confirm"),
  confirmCancel: document.getElementById("settings-advanced-cancel"),
  confirmTurnOn: document.getElementById("settings-advanced-turn-on"),
};

function settingsShowStatus(text, isError) {
  settingsUi.status.textContent = text;
  settingsUi.status.classList.toggle("settings-status-error", !!isError);
  settingsUi.status.classList.toggle("hidden", !text);
}

function settingsShowError(el, text) {
  el.textContent = text;
  el.classList.toggle("hidden", !text);
}

function settingsSetTogglesEnabled(enabled) {
  settingsUi.checkUpdates.disabled = !enabled;
  settingsUi.advanced.disabled = !enabled;
}

// "2026-11-19T14:03:00Z" -> "2026-11-19 14:03:00 UTC"
function settingsFormatUtc(utc) {
  if (!utc) return "";
  return String(utc).replace("T", " ").replace(/Z$/, " UTC");
}

function settingsBuildStampText(info) {
  const parts = [];
  if (info.git_sha) parts.push(info.git_sha + (info.dirty ? " (modified)" : ""));
  if (info.built_utc) parts.push("built " + settingsFormatUtc(info.built_utc));
  return parts.join(" · ") || "—";
}

function renderSettingsBuildInfo(result) {
  if (result.status === "fulfilled" && result.value) {
    settingsUi.buildVersion.textContent = result.value.version || "—";
    settingsUi.buildStamp.textContent = settingsBuildStampText(result.value);
  } else {
    settingsUi.buildVersion.textContent = "Unavailable";
    settingsUi.buildStamp.textContent = "Unavailable";
  }
}

function renderSettingsToggles() {
  settingsUi.checkUpdates.checked = settingsState.check_updates_on_launch;
  settingsUi.advanced.checked = settingsState.advanced_mode;
  settingsUi.advancedDetail.classList.toggle("hidden", !settingsState.advanced_mode);
}

function applyLoadedSettings(value) {
  settingsState.loaded = true;
  settingsState.advanced_mode = value.advanced_mode === true;
  settingsState.check_updates_on_launch = value.check_updates_on_launch !== false;
  notifySettingsChange();
}

async function loadSettingsPanel() {
  const mySeq = ++settingsUi.seq;
  settingsSetTogglesEnabled(false);
  settingsShowStatus("Loading settings…", false);
  settingsUi.buildVersion.textContent = "…";
  settingsUi.buildStamp.textContent = "…";
  const [settings, build] = await Promise.allSettled([
    invoke("get_settings"),
    invoke("get_build_info"),
  ]);
  if (mySeq !== settingsUi.seq || !settingsUi.open) return; // closed or reopened meanwhile
  renderSettingsBuildInfo(build);
  if (settings.status === "fulfilled" && settings.value) {
    applyLoadedSettings(settings.value);
    renderSettingsToggles();
    settingsSetTogglesEnabled(true);
    settingsShowStatus("", false);
  } else {
    const reason = settings.status === "rejected" ? String(settings.reason) : "no reply";
    settingsShowStatus(
      "Couldn't load settings: " + reason + ". Close Settings and open it again to retry.",
      true,
    );
  }
  // Focus moved to the panel on open; land on the first control once it's usable.
  if (document.activeElement === settingsUi.panel && !settingsUi.checkUpdates.disabled) {
    settingsUi.checkUpdates.focus();
  }
}

function openSettings() {
  if (settingsUi.open) return;
  if (rescue.state !== "closed" || repair.state !== "closed" || diag.state !== "closed") return; // one flow at a time
  settingsUi.open = true;
  hideAdvancedConfirm(false);
  settingsShowError(settingsUi.checkUpdatesError, "");
  settingsShowError(settingsUi.advancedError, "");
  settingsUi.overlay.classList.remove("hidden");
  settingsUi.container.inert = true; // keep Tab inside the panel
  settingsUi.panel.focus();
  loadSettingsPanel();
}

function closeSettings() {
  if (!settingsUi.open) return;
  settingsUi.open = false;
  settingsUi.seq++; // drop a load still on its way
  hideAdvancedConfirm(false);
  settingsUi.overlay.classList.add("hidden");
  settingsUi.container.inert = false;
  settingsUi.gear.focus();
}

// Called when the uninstall / repair modal closes: if Settings is still
// open underneath, put focus back on the button that opened the modal.
function settingsRefocus(id) {
  if (!settingsUi.open) return;
  const el = document.getElementById(id);
  if (el) el.focus();
}

// Saves one setting. On failure the checkbox goes back to `previous` and the
// error is shown under it.
async function saveSetting(command, key, value, input, errorEl) {
  const previous = settingsState[key];
  settingsShowError(errorEl, "");
  input.disabled = true;
  try {
    await invoke(command, { enabled: value });
    settingsState[key] = value;
    notifySettingsChange();
  } catch (e) {
    settingsState[key] = previous;
    settingsShowError(errorEl, "Couldn't save: " + e);
  }
  renderSettingsToggles();
  input.disabled = false;
}

function showAdvancedConfirm() {
  settingsUi.confirm.classList.remove("hidden");
  settingsUi.advanced.disabled = true;
  settingsUi.confirmCancel.focus();
}

function hideAdvancedConfirm(refocus) {
  const wasOpen = !settingsUi.confirm.classList.contains("hidden");
  settingsUi.confirm.classList.add("hidden");
  if (!wasOpen) return;
  settingsUi.advanced.disabled = !settingsState.loaded;
  if (refocus) settingsUi.advanced.focus();
}

function onAdvancedToggle() {
  if (settingsUi.advanced.checked) {
    // Turning on needs the confirm first; stay off until "Turn on".
    settingsUi.advanced.checked = false;
    settingsShowError(settingsUi.advancedError, "");
    showAdvancedConfirm();
    return;
  }
  saveSetting(
    "set_advanced_mode",
    "advanced_mode",
    false,
    settingsUi.advanced,
    settingsUi.advancedError,
  );
}

async function confirmAdvancedOn() {
  hideAdvancedConfirm(true);
  await saveSetting(
    "set_advanced_mode",
    "advanced_mode",
    true,
    settingsUi.advanced,
    settingsUi.advancedError,
  );
}

settingsUi.gear.addEventListener("click", openSettings);
document.getElementById("settings-close").addEventListener("click", closeSettings);
listen("settings-open", () => openSettings()).catch((e) =>
  console.error("[settings-open listen failed]", e),
);
settingsUi.checkUpdates.addEventListener("change", () =>
  saveSetting(
    "set_check_updates_on_launch",
    "check_updates_on_launch",
    settingsUi.checkUpdates.checked,
    settingsUi.checkUpdates,
    settingsUi.checkUpdatesError,
  ),
);
settingsUi.advanced.addEventListener("change", onAdvancedToggle);
settingsUi.confirmCancel.addEventListener("click", () => hideAdvancedConfirm(true));
settingsUi.confirmTurnOn.addEventListener("click", confirmAdvancedOn);
document.getElementById("settings-repair").addEventListener("click", openRepair);
document.getElementById("settings-uninstall").addEventListener("click", openRescue);
document.getElementById("settings-copy-diagnostics").addEventListener("click", openDiagnostics);

// Esc: the uninstall / repair / diagnostics modals handle their own Esc (they run first and
// call preventDefault). If the update modal popped up over Settings, Esc does
// nothing here (same as outside Settings: no dismiss, no snooze) and Settings
// stays open. Otherwise the Advanced confirm (= Cancel), then Settings itself.
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || !settingsUi.open) return;
  if (e.defaultPrevented || rescue.state !== "closed" || repair.state !== "closed" || diag.state !== "closed") return;
  if (updateModal.overlay && !updateModal.overlay.classList.contains("hidden")) return;
  e.preventDefault();
  if (!settingsUi.confirm.classList.contains("hidden")) {
    hideAdvancedConfirm(true);
    return;
  }
  closeSettings();
});

// Initialize once the DOM is ready
document.addEventListener("DOMContentLoaded", () => updateModal.init());

// These two must always be last
loadInterfaces();
loadQuickSets();

// --- Launch-time update check ---
//
// Runs after a short delay so it doesn't compete with the rest of the app's
// startup (loading interfaces, restoring last-used interface, etc).
//
// Network call happens on every launch unless "Check for updates when the
// app opens" is off in Settings — catches the "no internet on first launch
// of the day, internet on second launch" case.
//
// Popup suppression is separate: if the user dismissed the popup within
// the last 24 hours, we silently skip showing it even if there's an update.

const POPUP_SUPPRESSION_WINDOW_SECS = 24 * 60 * 60;
const LAUNCH_CHECK_DELAY_MS = 1000;

async function runLaunchUpdateCheck() {
  const { invoke } = window.__TAURI__.core;

  // Settings → General: "Check for updates when the app opens". If the
  // setting can't be read, check as before.
  try {
    const settings = await invoke("get_settings");
    if (settings && settings.check_updates_on_launch === false) return;
  } catch (e) {
    console.warn("[get_settings failed, checking for updates anyway]", e);
  }

  try {
    const info = await invoke("check_for_update");

    // No update available → nothing to do
    if (!info.update_available) {
      return;
    }

    // Update available — check whether the user dismissed it recently
    const dismissedAt = await invoke("get_update_prompt_dismissed_at");
    if (dismissedAt !== null && dismissedAt !== undefined) {
      const nowSecs = Math.floor(Date.now() / 1000);
      if (nowSecs - dismissedAt < POPUP_SUPPRESSION_WINDOW_SECS) {
        // Within the suppression window — silently skip
        return;
      }
    }

    // Show the popup
    updateModal.show(info);
  } catch (e) {
    // Silent failure — no internet, GitHub down, repo private, etc.
    // Console-log for dev visibility; in release, this is invisible to users.
    console.warn("[update check failed]", e);
  }
}

// Wait for DOM ready, then add a small delay so we don't run during the
// initial render flurry.
document.addEventListener("DOMContentLoaded", () => {
  setTimeout(runLaunchUpdateCheck, LAUNCH_CHECK_DELAY_MS);
});