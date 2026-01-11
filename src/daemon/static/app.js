// API Configuration
const API_BASE = window.location.pathname.replace(/\/+$/, '').replace(/\/app$/, '');
const API_TOKEN = null; // Set if authentication is required

// API Helper Functions
async function apiRequest(endpoint, options = {}) {
    const url = `${API_BASE}${endpoint}`;
    const headers = {
        'Content-Type': 'application/json',
        ...options.headers
    };
    
    if (API_TOKEN) {
        headers['token'] = API_TOKEN;
    }
    
    try {
        const response = await fetch(url, {
            ...options,
            headers
        });
        
        if (!response.ok) {
            throw new Error(`HTTP error! status: ${response.status}`);
        }
        
        return await response.json();
    } catch (error) {
        console.error('API request failed:', error);
        throw error;
    }
}

// Process Management Functions
async function listProcesses() {
    return await apiRequest('/list');
}

async function getProcessInfo(id) {
    return await apiRequest(`/process/${id}/info`);
}

async function createProcess(data) {
    return await apiRequest('/process/create', {
        method: 'POST',
        body: JSON.stringify(data)
    });
}

async function performAction(id, action) {
    return await apiRequest(`/process/${id}/action`, {
        method: 'POST',
        body: JSON.stringify({ method: action })
    });
}

async function getProcessLogs(id, type = 'out') {
    return await apiRequest(`/process/${id}/logs/${type}`);
}

async function renameProcess(id, name) {
    return await apiRequest(`/process/${id}/rename`, {
        method: 'POST',
        headers: {
            'Content-Type': 'text/plain'
        },
        body: name
    });
}

// UI Helper Functions
function formatUptime(startedAt) {
    const start = new Date(startedAt);
    const now = new Date();
    const diff = Math.floor((now - start) / 1000);
    
    if (diff < 60) return `${diff}s`;
    if (diff < 3600) return `${Math.floor(diff / 60)}m`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
    return `${Math.floor(diff / 86400)}d`;
}

function formatMemory(bytes) {
    if (!bytes) return 'N/A';
    const kb = bytes / 1024;
    if (kb < 1024) return `${kb.toFixed(0)}K`;
    const mb = kb / 1024;
    if (mb < 1024) return `${mb.toFixed(1)}M`;
    const gb = mb / 1024;
    return `${gb.toFixed(2)}G`;
}

function formatCPU(percent) {
    if (percent === null || percent === undefined) return 'N/A';
    return `${percent.toFixed(1)}%`;
}

// UI Rendering Functions
function renderProcessList(processes) {
    const container = document.getElementById('process-list');
    
    if (!processes || processes.length === 0) {
        container.innerHTML = `
            <div class="empty-state">
                <h3>No processes running</h3>
                <p>Start a new process to get started</p>
            </div>
        `;
        return;
    }
    
    container.innerHTML = processes.map(process => `
        <div class="process-item" data-process-id="${process.id}">
            <div class="process-header">
                <div class="process-info">
                    <div class="process-name">${escapeHtml(process.name)}</div>
                    <div class="process-script">${escapeHtml(process.script)}</div>
                </div>
                <div class="process-status ${process.running ? 'status-online' : 'status-stopped'}">
                    <span class="status-dot"></span>
                    ${process.running ? 'Running' : 'Stopped'}
                </div>
            </div>
            <div class="process-meta">
                <div class="process-meta-item">
                    <span>PID: ${process.pid || 'N/A'}</span>
                </div>
                <div class="process-meta-item">
                    <span>Uptime: ${process.running && process.started ? formatUptime(process.started) : 'N/A'}</span>
                </div>
                <div class="process-meta-item">
                    <span>CPU: ${formatCPU(process.stats?.cpu_percent)}</span>
                </div>
                <div class="process-meta-item">
                    <span>Memory: ${formatMemory(process.stats?.memory_usage?.rss)}</span>
                </div>
                <div class="process-meta-item">
                    <span>Restarts: ${process.restarts || 0}</span>
                </div>
            </div>
            <div class="process-actions">
                ${process.running ? `
                    <button class="btn btn-sm btn-secondary" onclick="restartProcess(${process.id})">Restart</button>
                    <button class="btn btn-sm btn-danger" onclick="stopProcess(${process.id})">Stop</button>
                ` : `
                    <button class="btn btn-sm btn-success" onclick="startProcess(${process.id})">Start</button>
                `}
                <button class="btn btn-sm btn-secondary" onclick="viewLogs(${process.id}, '${escapeHtml(process.name)}')">Logs</button>
                <button class="btn btn-sm btn-danger" onclick="removeProcess(${process.id})">Remove</button>
            </div>
        </div>
    `).join('');
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

// Process Action Functions
async function startProcess(id) {
    try {
        await performAction(id, 'start');
        showNotification('Process started successfully', 'success');
        await refreshProcessList();
    } catch (error) {
        showNotification('Failed to start process: ' + error.message, 'error');
    }
}

async function stopProcess(id) {
    if (!confirm('Are you sure you want to stop this process?')) return;
    
    try {
        await performAction(id, 'stop');
        showNotification('Process stopped successfully', 'success');
        await refreshProcessList();
    } catch (error) {
        showNotification('Failed to stop process: ' + error.message, 'error');
    }
}

async function restartProcess(id) {
    try {
        await performAction(id, 'restart');
        showNotification('Process restarted successfully', 'success');
        await refreshProcessList();
    } catch (error) {
        showNotification('Failed to restart process: ' + error.message, 'error');
    }
}

async function removeProcess(id) {
    if (!confirm('Are you sure you want to remove this process?')) return;
    
    try {
        await performAction(id, 'remove');
        showNotification('Process removed successfully', 'success');
        await refreshProcessList();
    } catch (error) {
        showNotification('Failed to remove process: ' + error.message, 'error');
    }
}

async function refreshProcessList() {
    try {
        const processes = await listProcesses();
        renderProcessList(processes);
    } catch (error) {
        console.error('Failed to refresh process list:', error);
        showNotification('Failed to load processes: ' + error.message, 'error');
    }
}

// Modal Functions
function showModal(modalId) {
    const modal = document.getElementById(modalId);
    modal.classList.add('show');
}

function hideModal(modalId) {
    const modal = document.getElementById(modalId);
    modal.classList.remove('show');
}

// New Process Form
async function handleNewProcessSubmit(event) {
    event.preventDefault();
    
    const form = event.target;
    const formData = new FormData(form);
    
    const data = {
        name: formData.get('name') || null,
        script: formData.get('script'),
        path: formData.get('path') || process.cwd || '/tmp',
        watch: formData.get('watch') || null
    };
    
    try {
        await createProcess(data);
        showNotification('Process created successfully', 'success');
        form.reset();
        hideModal('new-process-modal');
        await refreshProcessList();
    } catch (error) {
        showNotification('Failed to create process: ' + error.message, 'error');
    }
}

// Logs Viewer
let currentLogProcessId = null;
let logsFollowInterval = null;

async function viewLogs(id, name) {
    currentLogProcessId = id;
    document.getElementById('logs-title').textContent = `Logs: ${name}`;
    await loadLogs();
    showModal('logs-modal');
}

async function loadLogs() {
    if (!currentLogProcessId) return;
    
    const type = document.getElementById('logs-type').value;
    const logsContent = document.getElementById('logs-content');
    
    try {
        const result = await getProcessLogs(currentLogProcessId, type);
        const logs = result.logs || [];
        logsContent.textContent = logs.length > 0 ? logs.join('\n') : 'No logs available';
        
        // Auto-scroll to bottom
        logsContent.scrollTop = logsContent.scrollHeight;
    } catch (error) {
        logsContent.textContent = 'Failed to load logs: ' + error.message;
    }
}

function toggleLogsFollow() {
    const btn = document.getElementById('logs-follow-btn');
    
    if (logsFollowInterval) {
        clearInterval(logsFollowInterval);
        logsFollowInterval = null;
        btn.textContent = 'Follow Logs';
        btn.classList.remove('btn-primary');
    } else {
        logsFollowInterval = setInterval(loadLogs, 2000);
        btn.textContent = 'Stop Following';
        btn.classList.add('btn-primary');
        loadLogs();
    }
}

// Notifications
function showNotification(message, type = 'info') {
    // Simple console notification for now
    // Could be replaced with a proper notification system
    console.log(`[${type.toUpperCase()}] ${message}`);
    
    // Show browser alert for errors
    if (type === 'error') {
        alert(message);
    }
}

// Auto-refresh
let autoRefreshInterval = null;

function startAutoRefresh() {
    if (autoRefreshInterval) return;
    autoRefreshInterval = setInterval(refreshProcessList, 5000);
}

function stopAutoRefresh() {
    if (autoRefreshInterval) {
        clearInterval(autoRefreshInterval);
        autoRefreshInterval = null;
    }
}

// Event Listeners
document.addEventListener('DOMContentLoaded', () => {
    // Initial load
    refreshProcessList();
    startAutoRefresh();
    
    // Header buttons
    document.getElementById('refresh-btn').addEventListener('click', refreshProcessList);
    document.getElementById('new-process-btn').addEventListener('click', () => {
        showModal('new-process-modal');
    });
    
    // New process modal
    document.getElementById('close-modal-btn').addEventListener('click', () => {
        hideModal('new-process-modal');
    });
    document.getElementById('cancel-btn').addEventListener('click', () => {
        hideModal('new-process-modal');
    });
    document.getElementById('new-process-form').addEventListener('submit', handleNewProcessSubmit);
    
    // Logs modal
    document.getElementById('close-logs-btn').addEventListener('click', () => {
        hideModal('logs-modal');
        if (logsFollowInterval) {
            clearInterval(logsFollowInterval);
            logsFollowInterval = null;
        }
    });
    document.getElementById('logs-follow-btn').addEventListener('click', toggleLogsFollow);
    document.getElementById('logs-refresh-btn').addEventListener('click', loadLogs);
    document.getElementById('logs-type').addEventListener('change', loadLogs);
    
    // Close modals on background click
    document.querySelectorAll('.modal').forEach(modal => {
        modal.addEventListener('click', (e) => {
            if (e.target === modal) {
                hideModal(modal.id);
            }
        });
    });
    
    // Stop auto-refresh when page is hidden
    document.addEventListener('visibilitychange', () => {
        if (document.hidden) {
            stopAutoRefresh();
        } else {
            startAutoRefresh();
        }
    });
});

// Cleanup on page unload
window.addEventListener('beforeunload', () => {
    stopAutoRefresh();
    if (logsFollowInterval) {
        clearInterval(logsFollowInterval);
    }
});
