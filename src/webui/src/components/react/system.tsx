import { api } from '@/api';
import { useEffect, Fragment, useState, useRef } from 'react';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import ToastContainer from '@/components/react/toast';
import { useToast } from '@/components/react/useToast';
import { classNames, startDuration } from '@/helpers';

interface SystemInfo {
	hostname: string;
	os_type: string;
	os_version: string;
	cpu_count: number;
	total_memory: number;
	available_memory: number;
	used_memory: number;
	memory_percent: number;
	uptime: number;
	process_count: number;
	cpu_usage: number | null;
	disk_total: number | null;
	disk_free: number | null;
	disk_percent: number | null;
	load_avg_1: number | null;
	load_avg_5: number | null;
	load_avg_15: number | null;
}

interface ProcessItem {
	id: string;
	name: string;
	pid: number | null;
	running: boolean;
	cpu: string | null;
	memory: string | null;
	uptime: string | null;
}

const SystemPage = (props: { base: string }) => {
	const { toasts, closeToast, error } = useToast();
	const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
	const [processes, setProcesses] = useState<ProcessItem[]>([]);
	const [loading, setLoading] = useState(true);
	const [refreshingData, setRefreshingData] = useState(false);
	const isInitialMount = useRef(true);

	const badge = {
		running: 'bg-emerald-700/40 text-emerald-400',
		stopped: 'bg-red-700/40 text-red-400'
	};

	const formatBytes = (bytes: number): string => {
		const units = ['B', 'KB', 'MB', 'GB', 'TB'];
		let size = bytes;
		let unitIndex = 0;
		
		while (size >= 1024 && unitIndex < units.length - 1) {
			size /= 1024;
			unitIndex++;
		}
		
		return `${size.toFixed(2)} ${units[unitIndex]}`;
	};

	const formatUptime = (seconds: number): string => {
		const days = Math.floor(seconds / 86400);
		const hours = Math.floor((seconds % 86400) / 3600);
		const minutes = Math.floor((seconds % 3600) / 60);
		
		const parts = [];
		if (days > 0) parts.push(`${days}d`);
		if (hours > 0) parts.push(`${hours}h`);
		if (minutes > 0) parts.push(`${minutes}m`);
		
		return parts.length > 0 ? parts.join(' ') : 'Just started';
	};

	const fetchSystemInfo = async () => {
		try {
			const response = await api.get(`${props.base}/daemon/system`).json<SystemInfo>();
			setSystemInfo(response);
		} catch (err) {
			error('Failed to fetch system info: ' + (err as Error).message);
		}
	};

	const fetchProcesses = async () => {
		try {
			const response = await api.get(`${props.base}/list`).json<ProcessItem[]>();
			setProcesses(response);
		} catch (err) {
			error('Failed to fetch processes: ' + (err as Error).message);
		}
	};

	const fetchData = async (initial: boolean = false) => {
		if (initial) {
			setLoading(true);
		} else {
			setRefreshingData(true);
		}
		
		try {
			await Promise.all([fetchSystemInfo(), fetchProcesses()]);
		} finally {
			if (initial) {
				setLoading(false);
				isInitialMount.current = false;
			} else {
				setRefreshingData(false);
			}
		}
	};

	useEffect(() => {
		fetchData(true);
		const interval = setInterval(() => fetchData(false), 5000); // Refresh every 5 seconds
		return () => clearInterval(interval);
	}, []);

	if (loading && isInitialMount.current) {
		return <Loader />;
	}

	return (
		<Fragment>
			<ToastContainer toasts={toasts} onClose={closeToast} />
			<Header name="System Information" description="Overview of system resources and status.">
				<div className="flex gap-2">
					<button
						type="button"
						onClick={() => fetchData(false)}
						disabled={refreshingData}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-zinc-800 bg-zinc-950 text-zinc-50 hover:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg disabled:opacity-50 disabled:cursor-not-allowed">
						{refreshingData ? 'Refreshing...' : 'Refresh'}
					</button>
				</div>
			</Header>

			{systemInfo && (
				<>
					{/* System Information Card */}
					<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6 mb-6">
						<h2 className="text-lg font-semibold text-zinc-200 mb-4">System Information</h2>
						<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
							<div>
								<div className="text-sm text-zinc-400 mb-1">Hostname</div>
								<div className="text-zinc-200">{systemInfo.hostname}</div>
							</div>
							<div>
								<div className="text-sm text-zinc-400 mb-1">Operating System</div>
								<div className="text-zinc-200">{systemInfo.os_type}</div>
							</div>
							<div>
								<div className="text-sm text-zinc-400 mb-1">OS Version</div>
								<div className="text-zinc-200">{systemInfo.os_version}</div>
							</div>
							<div>
								<div className="text-sm text-zinc-400 mb-1">CPU Cores</div>
								<div className="text-zinc-200">{systemInfo.cpu_count}</div>
							</div>
							<div>
								<div className="text-sm text-zinc-400 mb-1">Total Memory</div>
								<div className="text-zinc-200">{formatBytes(systemInfo.total_memory)}</div>
							</div>
							<div>
								<div className="text-sm text-zinc-400 mb-1">Uptime</div>
								<div className="text-zinc-200">{formatUptime(systemInfo.uptime)}</div>
							</div>
						</div>
					</div>

					{/* Resource Usage Card */}
					<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6 mb-6">
						<h2 className="text-lg font-semibold text-zinc-200 mb-4">Resource Usage</h2>
						<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
							{/* CPU Usage */}
							{systemInfo.cpu_usage != null && (
								<div>
									<div className="text-sm text-zinc-400 mb-1">CPU Usage</div>
									<div className="text-zinc-200 font-semibold">
										{systemInfo.cpu_usage.toFixed(1)}%
									</div>
								</div>
							)}

							{/* Memory Usage */}
							<div>
								<div className="text-sm text-zinc-400 mb-1">Memory Usage</div>
								<div className="text-zinc-200 font-semibold">
									{systemInfo.memory_percent.toFixed(1)}%
								</div>
								<div className="text-xs text-zinc-500 mt-1">
									{formatBytes(systemInfo.used_memory)} used
								</div>
							</div>

							{/* Disk Usage */}
							{systemInfo.disk_percent != null && (
								<div>
									<div className="text-sm text-zinc-400 mb-1">Disk Usage</div>
									<div className="text-zinc-200 font-semibold">
										{systemInfo.disk_percent.toFixed(1)}%
									</div>
									{systemInfo.disk_free != null && systemInfo.disk_total != null && (
										<div className="text-xs text-zinc-500 mt-1">
											{formatBytes(systemInfo.disk_free * 1024)} free of {formatBytes(systemInfo.disk_total * 1024)}
										</div>
									)}
								</div>
							)}

							{/* Load Average */}
							{(systemInfo.load_avg_1 != null || 
							  systemInfo.load_avg_5 != null || 
							  systemInfo.load_avg_15 != null) && (
								<div>
									<div className="text-sm text-zinc-400 mb-1">Load Average</div>
									<div className="text-zinc-200">
										<span className="font-semibold">{systemInfo.load_avg_1?.toFixed(2) ?? '?'}</span>
										{' / '}
										<span>{systemInfo.load_avg_5?.toFixed(2) ?? '?'}</span>
										{' / '}
										<span>{systemInfo.load_avg_15?.toFixed(2) ?? '?'}</span>
									</div>
									<div className="text-xs text-zinc-500 mt-1">1 / 5 / 15 min</div>
								</div>
							)}
						</div>
					</div>

					{/* Processes Section */}
					<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6">
						<h2 className="text-lg font-semibold text-zinc-200 mb-4">
							Managed Processes ({processes.length})
						</h2>
						
						{processes.length === 0 ? (
							<div className="text-center py-8">
								<div className="text-zinc-400">No processes running</div>
								<div className="text-zinc-500 text-sm mt-2">
									Processes managed by OPM will appear here
								</div>
							</div>
						) : (
							<table className="w-full whitespace-nowrap text-left">
								<thead className="border-b border-zinc-800 text-sm leading-6 text-zinc-400">
									<tr>
										<th scope="col" className="py-2 pl-4 pr-8 font-semibold">
											Name
										</th>
										<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold sm:table-cell">
											PID
										</th>
										<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold sm:table-cell">
											Status
										</th>
										<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold md:table-cell">
											CPU
										</th>
										<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold md:table-cell">
											Memory
										</th>
										<th scope="col" className="py-2 pl-0 pr-4 text-right font-semibold">
											Uptime
										</th>
									</tr>
								</thead>
								<tbody className="divide-y divide-zinc-800">
									{processes.map((process: ProcessItem) => (
										<tr key={process.id} className="hover:bg-zinc-800/30 transition">
											<td className="py-3 pl-4 pr-8">
												<div className="text-sm font-medium text-white">{process.name}</div>
											</td>
											<td className="hidden py-3 pl-0 pr-8 sm:table-cell">
												<div className="text-sm text-zinc-400 font-mono">{process.pid || 'N/A'}</div>
											</td>
											<td className="hidden py-3 pl-0 pr-8 sm:table-cell">
												<div className={classNames(
													process.running ? badge.running : badge.stopped,
													'inline-flex rounded-md px-2 py-1 text-xs font-medium ring-1 ring-inset ring-white/10'
												)}>
													{process.running ? 'Running' : 'Stopped'}
												</div>
											</td>
											<td className="hidden py-3 pl-0 pr-8 md:table-cell">
												<div className="text-sm text-zinc-400">
													{process.cpu || 'N/A'}
												</div>
											</td>
											<td className="hidden py-3 pl-0 pr-8 md:table-cell">
												<div className="text-sm text-zinc-400">
													{process.memory?.toUpperCase() || 'N/A'}
												</div>
											</td>
											<td className="py-3 pl-0 pr-4 text-right">
												<div className="text-sm text-zinc-400">
													{(process.uptime && startDuration(process.uptime, false)) || 'N/A'}
												</div>
											</td>
										</tr>
									))}
								</tbody>
							</table>
						)}
					</div>
				</>
			)}
		</Fragment>
	);
};

export default SystemPage;
