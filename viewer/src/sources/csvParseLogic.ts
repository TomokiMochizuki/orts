/**
 * CSV parse logic extracted as pure functions.
 *
 * Used by both the Web Worker (csvParseWorker.ts) and tests.
 * No DOM, Worker, or React dependencies.
 */

import { type CSVMetadata, type OrbitPoint, torqueModelsOf } from "../orbit.js";
import {
  type CSVColumns,
  emptyMetadata,
  parseDataLine,
  parseDataLineWithColumns,
  parseHeaderLine,
  parseMetadataLine,
} from "./parseCSVLine.js";

// Worker message protocol

export type CSVWorkerMessage =
  | { type: "metadata"; metadata: CSVMetadata }
  | { type: "chunk"; points: OrbitPoint[] }
  | {
      type: "complete";
      totalPoints: number;
      /** Models whose torque the file carries, per satellite id — known only
       * once the rows have been read. Absent from a worker build that
       * predates it, which reads as "none". */
      torqueModels?: Record<string, string[]>;
    }
  | { type: "error"; message: string };

// Chunked parser (pure function)

/**
 * Parse a CSV string in chunks, emitting messages via the callback.
 *
 * Message order: metadata → chunk* → complete
 *
 * @param text Full CSV text
 * @param chunkSize Maximum points per chunk message
 * @param emit Callback to emit messages (in Worker: postMessage)
 */
export function parseCSVChunked(
  text: string,
  chunkSize: number,
  emit: (msg: CSVWorkerMessage) => void,
): void {
  const metadata = emptyMetadata();
  const torqueModels = new Map<string, Set<string>>();
  const lines = text.split("\n");
  let totalPoints = 0;
  let chunk: OrbitPoint[] = [];

  // First pass: extract metadata from comment lines at the top. The column
  // header is written as a comment too, and reading it is what lets the
  // attitude and torque columns be found — they sit past where the positional
  // parsing stops, and a file may grow a column in the middle.
  let dataStart = 0;
  let columns: CSVColumns | null = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (line === "") continue;
    if (line.startsWith("#")) {
      const header = parseHeaderLine(line);
      if (header) {
        columns = header;
        continue;
      }
      parseMetadataLine(line, metadata);
      continue;
    }
    dataStart = i;
    break;
  }

  // Emit metadata first. What models the file carries a torque for is only
  // known once the rows have been read, so it goes out with the last chunk.
  emit({ type: "metadata", metadata });

  // Detect multi-satellite mode.
  // When `# satellites = ...` is present, the CSV always has a satellite_id
  // first column, even for single-satellite files (matches orts run output).
  const multiSat = metadata.satellites != null && metadata.satellites.length > 0;

  // Parse data lines in chunks
  for (let i = dataStart; i < lines.length; i++) {
    const line = lines[i].trim();
    if (line === "" || line.startsWith("#")) continue;

    // A file written before the header existed has none, and is read by
    // position as it always was.
    const point = columns ? parseDataLineWithColumns(line, columns) : parseDataLine(line, multiSat);
    if (!point) continue;
    torqueModelsOf([point], torqueModels);

    chunk.push(point);
    totalPoints++;

    if (chunk.length >= chunkSize) {
      emit({ type: "chunk", points: chunk });
      chunk = [];
    }
  }

  // Emit remaining points
  if (chunk.length > 0) {
    emit({ type: "chunk", points: chunk });
  }

  emit({
    type: "complete",
    totalPoints,
    torqueModels: Object.fromEntries([...torqueModels].map(([id, m]) => [id, [...m]])),
  });
}
