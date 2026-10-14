import React from 'react';
import { createRoot } from 'react-dom/client';
import { OrderForm } from './OrderForm.jsx';
function App() {
  return (<main style={{ fontFamily: 'system-ui', padding: 24 }}>
    <h1 className="page-title">Orders</h1>
    <OrderForm />
    <p className="spinner" style={{ animation: 'spin 2s linear infinite', display: 'inline-block' }}>⟳</p>
    <style>{'@keyframes spin{to{transform:rotate(360deg)}}'}</style>
  </main>);
}
createRoot(document.getElementById('root')).render(<App />);
