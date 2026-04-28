import { invoke } from "@tauri-apps/api/core";

const interfaceSelect = document.getElementById("interface-select");
const statusBar = document.getElementById("status-bar");
const currentIpLabel = document.getElementById("current-ip-label");
const currentSubnetLabel = document.getElementById("current-subnet-label");
const currentGatewayLabel = document.getElementById("current-gateway-label");
const currentDhcpLabel = document.getElementById("current-dhcp-label");

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
        info.subnet === set.data.subnet && info.gateway === set.data.gateway) {
      document.getElementById(set.el).classList.add("active-mode");
      display.style.borderLeftColor = "#0a84ff";
      return;
    }
  }

  if (previousIpInfo && info.ip === previousIpInfo.ip &&
      info.subnet === previousIpInfo.subnet && info.gateway === previousIpInfo.gateway) {
    document.getElementById("prev-ip").classList.add("active-mode");
    display.style.borderLeftColor = "#0a84ff";
    return;
  }

  // No match — neutral border
  display.style.borderLeftColor = "#444";
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
    statusBar.textContent = "Error: " + error;
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
    await loadIpInfo(iface);
  } catch (error) {
    statusBar.textContent = "Error: " + error;
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
    await loadIpInfo(iface);
  } catch (error) {
    statusBar.textContent = "Error: " + error;
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
    await loadIpInfo(iface);
  } catch (error) {
    statusBar.textContent = "Error: " + error;
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
    await loadIpInfo(iface);
  } catch (error) {
    statusBar.textContent = "Error: " + error;
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
    await loadIpInfo(iface);
  } catch (error) {
    statusBar.textContent = "Error: " + error;
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
    await loadIpInfo(iface);
  } catch (error) {
    statusBar.textContent = "Error: " + error;
  }
});

["manual-ip", "manual-subnet", "manual-gateway"].forEach(id => {
  document.getElementById(id).addEventListener("input", () => {
    document.getElementById(id).classList.remove("invalid");
  });
});

// These two must always be last
loadInterfaces();
loadQuickSets();