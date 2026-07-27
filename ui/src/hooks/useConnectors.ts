import { useState, useEffect, useCallback } from 'react';
import { ConnectorSummary } from '../api/types';
import { fetchConnectors, checkBackendHealth } from '../api/client';

export function useConnectors() {
  const [connectors, setConnectors] = useState<ConnectorSummary[]>([]);
  const [isConnected, setIsConnected] = useState<boolean>(false);
  const [loading, setLoading] = useState<boolean>(true);

  const loadConnectors = useCallback(async () => {
    setLoading(true);
    const healthy = await checkBackendHealth();
    setIsConnected(healthy);

    if (healthy) {
      try {
        const data = await fetchConnectors();
        setConnectors(data);
      } catch (err) {
        console.error('Failed to load connector summaries:', err);
        setConnectors([]);
      }
    } else {
      setConnectors([]);
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    loadConnectors();
    const interval = setInterval(loadConnectors, 30000);
    return () => clearInterval(interval);
  }, [loadConnectors]);

  return {
    connectors,
    isConnected,
    loading,
    refetch: loadConnectors,
  };
}
