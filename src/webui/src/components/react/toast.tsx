import { Fragment, useEffect } from 'react';
import { Transition } from '@headlessui/react';

export type ToastType = 'success' | 'error' | 'info';

export interface Toast {
	id: string;
	type: ToastType;
	message: string;
	duration?: number;
}

interface ToastItemProps {
	toast: Toast;
	onClose: (id: string) => void;
}

const ToastItem = ({ toast, onClose }: ToastItemProps) => {
	useEffect(() => {
		const timer = setTimeout(() => {
			onClose(toast.id);
		}, toast.duration || 5000);

		return () => clearTimeout(timer);
	}, [toast.id, toast.duration, onClose]);

	const bgColor = {
		success: 'bg-emerald-600',
		error: 'bg-red-600',
		info: 'bg-blue-600'
	}[toast.type];

	const icon = {
		success: (
			<svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
			</svg>
		),
		error: (
			<svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
			</svg>
		),
		info: (
			<svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
			</svg>
		)
	}[toast.type];

	return (
		<Transition
			show={true}
			as={Fragment}
			enter="transform ease-out duration-300 transition"
			enterFrom="translate-y-2 opacity-0 sm:translate-y-0 sm:translate-x-2"
			enterTo="translate-y-0 opacity-100 sm:translate-x-0"
			leave="transition ease-in duration-200"
			leaveFrom="opacity-100"
			leaveTo="opacity-0">
			<div className={`${bgColor} text-white px-4 py-3 rounded-lg shadow-lg flex items-center gap-3 min-w-[280px] max-w-[400px]`}>
				<div className="flex-shrink-0">{icon}</div>
				<div className="flex-1 text-sm font-medium">{toast.message}</div>
				<button
					onClick={() => onClose(toast.id)}
					className="flex-shrink-0 text-white/80 hover:text-white transition-colors">
					<svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
						<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
					</svg>
				</button>
			</div>
		</Transition>
	);
};

interface ToastContainerProps {
	toasts: Toast[];
	onClose: (id: string) => void;
}

const ToastContainer = ({ toasts, onClose }: ToastContainerProps) => {
	return (
		<>
			{/* Desktop: Bottom-right */}
			<div className="hidden sm:block fixed bottom-4 right-4 z-[9999] space-y-2 pointer-events-none">
				<div className="pointer-events-auto">
					{toasts.map((toast) => (
						<ToastItem key={toast.id} toast={toast} onClose={onClose} />
					))}
				</div>
			</div>

			{/* Mobile: Top-center */}
			<div className="sm:hidden fixed top-20 left-1/2 -translate-x-1/2 z-[9999] space-y-2 w-[calc(100vw-2rem)] pointer-events-none">
				<div className="pointer-events-auto">
					{toasts.map((toast) => (
						<ToastItem key={toast.id} toast={toast} onClose={onClose} />
					))}
				</div>
			</div>
		</>
	);
};

export default ToastContainer;
