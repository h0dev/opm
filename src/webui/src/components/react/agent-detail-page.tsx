import { useEffect, useState } from 'react';
import AgentDetail from '@/components/react/agent-detail';

const AgentDetailPage = (props: { base: string }) => {
	const [agentId, setAgentId] = useState<string>('');

	useEffect(() => {
		// Get agent ID from URL hash
		const hash = window.location.hash.substring(1);
		setAgentId(hash || '');

		// Listen for hash changes
		const handleHashChange = () => {
			const newHash = window.location.hash.substring(1);
			setAgentId(newHash || '');
		};

		window.addEventListener('hashchange', handleHashChange);
		return () => window.removeEventListener('hashchange', handleHashChange);
	}, []);

	// Wait for agent ID to be set from URL hash
	if (!agentId) {
		return null;
	}

	return <AgentDetail agentId={agentId} base={props.base} />;
};

export default AgentDetailPage;
