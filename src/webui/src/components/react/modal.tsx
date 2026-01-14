import { Fragment, useEffect } from 'react';
import { Dialog, DialogPanel, DialogTitle, DialogBackdrop, Transition, TransitionChild } from '@headlessui/react';

const Modal = (props: { show: boolean; callback: any; title: string; children: any }) => {
	// Prevent modal from closing when clicking inside input/textarea
	useEffect(() => {
		const handleClickOutside = (e: MouseEvent) => {
			// If the active element is an input or textarea, don't close the modal
			const activeEl = document.activeElement;
			if (activeEl && (activeEl.tagName === 'INPUT' || activeEl.tagName === 'TEXTAREA')) {
				e.stopPropagation();
				return;
			}
		};

		if (props.show) {
			document.addEventListener('mousedown', handleClickOutside, true);
		}

		return () => {
			document.removeEventListener('mousedown', handleClickOutside, true);
		};
	}, [props.show]);

	return (
		<Transition show={props.show} as={Fragment}>
			<Dialog as="div" className="fixed z-[300] inset-0 overflow-y-auto" onClose={() => props.callback(false)}>
				<div className="flex items-end justify-center min-h-screen pt-4 px-4 pb-20 text-center sm:block sm:p-0">
					<TransitionChild
						as={Fragment}
						enter="ease-out duration-300"
						enterFrom="opacity-0"
						enterTo="opacity-100"
						leave="ease-in duration-200"
						leaveFrom="opacity-100"
						leaveTo="opacity-0">
						<DialogBackdrop className="fixed inset-0 bg-black bg-opacity-60 transition-opacity" style={{ backdropFilter: 'blur(5px)' }} />
					</TransitionChild>
					<span className="hidden sm:inline-block sm:align-middle sm:h-screen" aria-hidden="true">
						&#8203;
					</span>
					<TransitionChild
						as={Fragment}
						enter="ease-out duration-300"
						enterFrom="opacity-0 translate-y-4 sm:translate-y-0 sm:scale-95"
						enterTo="opacity-100 translate-y-0 sm:scale-100"
						leave="ease-in duration-200"
						leaveFrom="opacity-100 translate-y-0 sm:scale-100"
						leaveTo="opacity-0 translate-y-4 sm:translate-y-0 sm:scale-95">
						<DialogPanel 
							className="inline-block align-bottom bg-zinc-950 border border-zinc-800 rounded-lg text-left overflow-hidden shadow-xl transform transition-all sm:my-8 sm:align-middle sm:max-w-lg sm:w-full"
							onClick={(e) => e.stopPropagation()}
							onTouchStart={(e) => e.stopPropagation()}
							onTouchEnd={(e) => e.stopPropagation()}>
							<div className="bg-zinc-950 px-4 pt-5 pb-4 sm:p-6 sm:pb-4">
								<div className="sm:flex sm:items-start">
									<div className="mt-3 text-center sm:mt-0 sm:text-left">
										<DialogTitle as="h3" className="text-3xl leading-6 font-bold text-zinc-300 mb-[1.5rem]">
											{props.title}
										</DialogTitle>
										<div className="mt-2">
											<span className="text-sm text-zinc-400">{props.children}</span>
										</div>
									</div>
								</div>
							</div>
						</DialogPanel>
					</TransitionChild>
				</div>
			</Dialog>
		</Transition>
	);
};

export default Modal;
