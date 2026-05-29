import { transport } from '../stores/transport';

export type PositionFromPointer = (clientX: number, altKey: boolean) => number;

/** Scrub transport : mise à jour optimiste + callback au mouseup (seek moteur). */
export function startTransportScrub(
	event: MouseEvent,
	positionFromPointer: PositionFromPointer,
	onEnd: (positionSec: number) => void,
): () => void {
	event.preventDefault();

	let lastPosition = positionFromPointer(event.clientX, event.altKey);
	transport.update((t) => ({ ...t, position: lastPosition }));

	const onMove = (e: MouseEvent) => {
		lastPosition = positionFromPointer(e.clientX, e.altKey);
		transport.update((t) => ({ ...t, position: lastPosition }));
	};

	const onUp = () => {
		cleanup();
		onEnd(lastPosition);
	};

	const cleanup = () => {
		window.removeEventListener('mousemove', onMove);
		window.removeEventListener('mouseup', onUp);
	};

	window.addEventListener('mousemove', onMove);
	window.addEventListener('mouseup', onUp);

	return cleanup;
}
