import { analyze } from './practice.js';
onmessage = ({ data }) => {
  try {
    postMessage({
      id: data.id,
      ...analyze(data.board, data.history, 3000, data.proof),
    });
  } catch (error) {
    postMessage({ id: data.id, error: error.message });
  }
};
