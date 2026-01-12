import ky from 'ky';
import { $settings } from '@/store';

export { SSE } from 'sse.js';

export const headers = { token: $settings.get().token };

// Retry configuration for API requests
const retryConfig = {
	limit: 2,
	methods: ['get'],
	statusCodes: [408, 413, 429, 500, 502, 503, 504]
};

// Create API client with optimized settings
export const api = ky.create({ 
	headers,
	timeout: 10000, // 10 second timeout
	retry: retryConfig
});
