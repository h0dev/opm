import { useEffect, useState } from "@astrojs/react";
import { api } from "@/api";
import { useToast } from "./useToast";

interface SystemInfo {
  hostname: string;
  os_type: string;
  os_version: string;
  arch: string;
  cpu_cores: number;
  total_memory: number;
}

function SystemPage({ base }: { base: string }) {
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const { toast } = useToast();

  useEffect(() => {
    fetchSystemInfo();
  }, []);

  const fetchSystemInfo = async () => {
    try {
      const data = await api.get(`${base}/daemon/system-info`).json<SystemInfo>();
      setSystemInfo(data);
    } catch (error) {
      console.error("Error fetching system info:", error);
      toast({
        type: "error",
        message: "Failed to load system information",
      });
    } finally {
      setLoading(false);
    }
  };

  const formatMemory = (bytes: number) => {
    const gb = bytes / (1024 * 1024 * 1024);
    return `${gb.toFixed(2)} GB`;
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-[60vh]">
        <div className="text-zinc-400 text-lg">Loading...</div>
      </div>
    );
  }

  return (
    <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
      <h1 className="text-3xl font-bold text-zinc-100 mb-8">System</h1>

      {/* System Information */}
      <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-6 mb-8">
        <h2 className="text-xl font-semibold text-zinc-100 mb-4">System Information</h2>
        {systemInfo ? (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div className="bg-zinc-800/50 rounded p-4">
              <div className="text-zinc-400 text-sm mb-1">Hostname</div>
              <div className="text-zinc-100 font-medium">{systemInfo.hostname}</div>
            </div>
            <div className="bg-zinc-800/50 rounded p-4">
              <div className="text-zinc-400 text-sm mb-1">Operating System</div>
              <div className="text-zinc-100 font-medium">{systemInfo.os_type} {systemInfo.os_version}</div>
            </div>
            <div className="bg-zinc-800/50 rounded p-4">
              <div className="text-zinc-400 text-sm mb-1">Architecture</div>
              <div className="text-zinc-100 font-medium">{systemInfo.arch}</div>
            </div>
            <div className="bg-zinc-800/50 rounded p-4">
              <div className="text-zinc-400 text-sm mb-1">CPU Cores</div>
              <div className="text-zinc-100 font-medium">{systemInfo.cpu_cores}</div>
            </div>
            <div className="bg-zinc-800/50 rounded p-4">
              <div className="text-zinc-400 text-sm mb-1">Total Memory</div>
              <div className="text-zinc-100 font-medium">{formatMemory(systemInfo.total_memory)}</div>
            </div>
          </div>
        ) : (
          <div className="text-zinc-400">Failed to load system information</div>
        )}
      </div>

      {/* System Usage - Placeholder for future implementation */}
      <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-6">
        <h2 className="text-xl font-semibold text-zinc-100 mb-4">System Usage</h2>
        <div className="text-zinc-400 text-center py-8">
          System usage monitoring will be available in a future update
        </div>
      </div>
    </div>
  );
}

export default SystemPage;
