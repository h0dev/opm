import { useEffect, useState } from "@astrojs/react";
import { api } from "@/api";
import { useToast } from "./useToast";

interface Event {
  id: number;
  timestamp: string;
  event_type: 'process_start' | 'process_stop' | 'process_crash' | 'process_restart' | 'agent_connect' | 'agent_disconnect';
  title: string;
  message: string;
}

function EventsPage({ base }: { base: string }) {
  const [events, setEvents] = useState<Event[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedType, setSelectedType] = useState<string>("all");
  const { toast } = useToast();

  useEffect(() => {
    fetchEvents();
    // Refresh events every 5 seconds
    const interval = setInterval(() => {
      fetchEvents();
    }, 5000);
    return () => clearInterval(interval);
  }, []);

  const fetchEvents = async () => {
    try {
      const data = await api.get(`${base}/daemon/events`).json<Event[]>();
      setEvents(data);
      if (loading) setLoading(false);
    } catch (error) {
      console.error("Error fetching events:", error);
      if (loading) {
        toast({
          type: "error",
          message: "Failed to load events",
        });
        setLoading(false);
      }
    }
  };

  const filteredEvents = selectedType === "all" 
    ? events 
    : events.filter(e => e.event_type === selectedType);

  const getEventIcon = (type: string) => {
    switch (type) {
      case "process_start":
        return "▶️";
      case "process_stop":
        return "⏹️";
      case "process_crash":
        return "💥";
      case "process_restart":
        return "🔄";
      case "agent_connect":
        return "🔗";
      case "agent_disconnect":
        return "🔌";
      default:
        return "📝";
    }
  };

  const getEventColor = (type: string) => {
    switch (type) {
      case "process_start":
        return "text-emerald-400";
      case "process_stop":
        return "text-zinc-400";
      case "process_crash":
        return "text-red-400";
      case "process_restart":
        return "text-blue-400";
      case "agent_connect":
        return "text-green-400";
      case "agent_disconnect":
        return "text-orange-400";
      default:
        return "text-zinc-400";
    }
  };

  const formatTimestamp = (timestamp: string) => {
    const date = new Date(timestamp);
    return date.toLocaleString();
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-[60vh]">
        <div className="text-zinc-400 text-lg">Loading events...</div>
      </div>
    );
  }

  return (
    <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
      <h1 className="text-3xl font-bold text-zinc-100 mb-8">Events</h1>

      {/* Events */}
      <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-6">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-xl font-semibold text-zinc-100">Event Log</h2>
          <select
            value={selectedType}
            onChange={(e) => setSelectedType(e.target.value)}
            className="bg-zinc-800 text-zinc-100 border border-zinc-700 rounded px-3 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-sky-500"
          >
            <option value="all">All Events</option>
            <option value="process_start">Process Start</option>
            <option value="process_stop">Process Stop</option>
            <option value="process_crash">Process Crash</option>
            <option value="process_restart">Process Restart</option>
            <option value="agent_connect">Agent Connect</option>
            <option value="agent_disconnect">Agent Disconnect</option>
          </select>
        </div>

        {filteredEvents.length === 0 ? (
          <div className="text-center py-12 text-zinc-400">
            No events to display
          </div>
        ) : (
          <div className="space-y-2 max-h-[600px] overflow-y-auto">
            {filteredEvents.reverse().map((event) => (
              <div
                key={event.id}
                className="bg-zinc-800/50 rounded-lg p-4 hover:bg-zinc-800 transition-colors"
              >
                <div className="flex items-start gap-3">
                  <span className="text-2xl flex-shrink-0">{getEventIcon(event.event_type)}</span>
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 mb-1">
                      <h3 className={`font-medium ${getEventColor(event.event_type)}`}>
                        {event.title}
                      </h3>
                      <span className="text-xs text-zinc-500">
                        {formatTimestamp(event.timestamp)}
                      </span>
                    </div>
                    <p className="text-zinc-300 text-sm">{event.message}</p>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

export default EventsPage;
