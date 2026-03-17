// OpenFang Simulations Page — MiroFish swarm-intelligence integration
'use strict';

function simulationsPage() {
  return {
    tab: 'status',
    connected: false,
    baseUrl: '',
    version: '',

    // Local folder analyzer
    analyzeForm: { folder_path: '', topic: '', rounds: null, mode: 'fast' },
    analyzing: false,
    analyzeResult: null,
    analyzeError: '',

    // Graph builder
    graphForm: { project_name: '', documents: '' },
    graphBuilding: false,
    graphResult: null,
    graphError: '',

    // Simulation runner
    simForm: { graph_id: '', topic: '', rounds: null },
    simRunning: false,
    simStatus: null,
    simError: '',
    pollTimer: null,

    // Interview
    interviewForm: { simulation_id: '', agent_name: '', question: '' },
    agents: [],
    agentsLoading: false,
    interviewing: false,
    interviewResult: null,
    interviewError: '',

    // Reports
    reportForm: { simulation_id: '', topic: '' },
    reportGenerating: false,
    reportResult: null,
    reportError: '',

    async checkHealth() {
      try {
        const r = await fetch('/api/mirofish/health');
        const d = await r.json();
        this.connected = d.reachable || false;
        this.version = d.version || '';
        this.baseUrl = d.base_url || '(from config)';
      } catch {
        this.connected = false;
      }
    },

    async analyzeFolder() {
      this.analyzing = true;
      this.analyzeError = '';
      this.analyzeResult = null;
      try {
        const payload = {
          folder_path: (this.analyzeForm.folder_path || '').trim(),
          topic: (this.analyzeForm.topic || '').trim(),
          mode: this.analyzeForm.mode || 'fast'
        };
        if (this.analyzeForm.rounds) payload.rounds = this.analyzeForm.rounds;

        const r = await fetch('/api/mirofish/analyze-folder', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        });
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Analyze folder failed');

        this.analyzeResult = d;
        if (d.graph_id) this.simForm.graph_id = d.graph_id;
        if (d.simulation_id) {
          this.interviewForm.simulation_id = d.simulation_id;
          this.reportForm.simulation_id = d.simulation_id;
        }
        if (d.report && d.report.content) this.reportResult = d.report;
      } catch (e) {
        this.analyzeError = e.message;
      } finally {
        this.analyzing = false;
      }
    },

    async buildGraph() {
      this.graphBuilding = true;
      this.graphError = '';
      this.graphResult = null;
      try {
        const docs = this.graphForm.documents.split('\n').filter(l => l.trim());
        const r = await fetch('/api/mirofish/graph/build', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ project_name: this.graphForm.project_name, documents: docs })
        });
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Build failed');
        this.graphResult = d;
        this.simForm.graph_id = d.graph_id || '';
      } catch (e) {
        this.graphError = e.message;
      } finally {
        this.graphBuilding = false;
      }
    },

    async createSimulation() {
      this.simRunning = true;
      this.simError = '';
      this.simStatus = null;
      try {
        const body = { graph_id: this.simForm.graph_id, topic: this.simForm.topic };
        if (this.simForm.rounds) body.rounds = this.simForm.rounds;
        const r = await fetch('/api/mirofish/simulations', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body)
        });
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Create failed');
        this.simStatus = d;
        this.interviewForm.simulation_id = d.id || '';
        this.reportForm.simulation_id = d.id || '';
        // Start polling if not completed
        if (d.status !== 'completed' && d.status !== 'failed') {
          this.startPolling(d.id);
        }
      } catch (e) {
        this.simError = e.message;
      } finally {
        this.simRunning = false;
      }
    },

    startPolling(simId) {
      if (this.pollTimer) clearInterval(this.pollTimer);
      this.pollTimer = setInterval(async () => {
        try {
          const r = await fetch('/api/mirofish/simulations/' + simId + '/status');
          const d = await r.json();
          if (r.ok) {
            this.simStatus = d;
            if (d.status === 'completed' || d.status === 'failed') {
              clearInterval(this.pollTimer);
              this.pollTimer = null;
            }
          }
        } catch { /* retry next tick */ }
      }, 3000);
    },

    async loadAgents() {
      this.agentsLoading = true;
      this.agents = [];
      try {
        const r = await fetch('/api/mirofish/graph/' + this.interviewForm.simulation_id + '/entities');
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Load failed');
        this.agents = d;
        if (d.length) this.interviewForm.agent_name = d[0].name;
      } catch (e) {
        this.interviewError = e.message;
      } finally {
        this.agentsLoading = false;
      }
    },

    async interview() {
      this.interviewing = true;
      this.interviewError = '';
      this.interviewResult = null;
      try {
        const r = await fetch('/api/mirofish/simulations/' + this.interviewForm.simulation_id + '/interview', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ agent_name: this.interviewForm.agent_name, question: this.interviewForm.question })
        });
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Interview failed');
        this.interviewResult = d;
      } catch (e) {
        this.interviewError = e.message;
      } finally {
        this.interviewing = false;
      }
    },

    async generateReport() {
      this.reportGenerating = true;
      this.reportError = '';
      this.reportResult = null;
      try {
        const body = { simulation_id: this.reportForm.simulation_id };
        if (this.reportForm.topic) body.topic = this.reportForm.topic;
        const r = await fetch('/api/mirofish/simulations/' + this.reportForm.simulation_id + '/report', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body)
        });
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Report generation failed');
        this.reportResult = d;
      } catch (e) {
        this.reportError = e.message;
      } finally {
        this.reportGenerating = false;
      }
    }
  };
}
