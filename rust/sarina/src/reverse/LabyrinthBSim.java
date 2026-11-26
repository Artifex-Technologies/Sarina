// LabyrinthBSim: compares the program Ghidra just analysed with the similarity library (a local BSim H2 database), then
// adds the program's signatures to it. Run by Sarina's reverse_engineer command "similar", inside the labyrinth-rea
// sandbox (no network, the file read-only), as a post-script of Ghidra's headless analyzer on a read-only import:
//
//   analyzeHeadless DIR p -import FILE -readOnly -deleteProject -scriptPath DIR -postScript LabyrinthBSim.java LIBRARY OUT
//
// LIBRARY is the database file (…/library.mv.db), OUT the file the report is written to. Nothing is ever run: the
// signatures come from Ghidra's decompiler. Names in the report come from the files themselves (symbol tables of possibly
// hostile programs), so they are cut and reduced to plain characters; the report is bounded.
//
// Part of Sarina (MPL-2.0). Uses Ghidra's BSim API (Apache-2.0), as Ghidra's own QueryFunction.java and
// AddProgramToH2BSimDatabaseScript.java do.
import java.io.File;
import java.io.PrintWriter;
import java.util.*;

import generic.lsh.vector.LSHVectorFactory;
import ghidra.app.script.GhidraScript;
import ghidra.features.bsim.query.*;
import ghidra.features.bsim.query.FunctionDatabase.BSimError;
import ghidra.features.bsim.query.FunctionDatabase.ErrorCategory;
import ghidra.features.bsim.query.description.*;
import ghidra.features.bsim.query.protocol.*;
import ghidra.program.model.listing.FunctionManager;

public class LabyrinthBSim extends GhidraScript {

	static final double SIMILARITY = 0.7; // a function pair below this is not reported
	static final double SELF_SIGNIFICANCE = 15.0; // functions too small to say anything (stubs) are not queried (as Ghidra's LocalBSimQueryScript)
	// code the C toolchain puts in every program: alike everywhere, it says nothing about where a program comes from
	// (_INIT_0 and _FINI_0: Ghidra's names for frame_dummy and __do_global_dtors_aux in a stripped program)
	static final Set<String> TOOLCHAIN = Set.of("_start", "_init", "_fini", "__do_global_dtors_aux", "frame_dummy",
		"register_tm_clones", "deregister_tm_clones", "__libc_csu_init", "__libc_csu_fini", "_dl_relocate_static_pie",
		"_INIT_0", "_FINI_0");
	static final int PER_FUNCTION = 3, MAX_EXES = 10, MAX_PAIRS = 25, MAX_NAME = 80;

	static String plain(String s) {
		if (s == null) {
			return "?";
		}
		StringBuilder b = new StringBuilder();
		for (char c : s.toCharArray()) {
			if (b.length() == MAX_NAME) {
				b.append("...");
				break;
			}
			b.append(c >= 0x21 && c < 0x7f ? c : '_');
		}
		return b.toString();
	}

	static class Exe {
		String name, md5;
		int matched;
		double best, significance;
	}

	@Override
	protected void run() throws Exception {
		String[] a = getScriptArgs();
		if (a.length != 2 || currentProgram == null) {
			throw new IllegalArgumentException("usage: LabyrinthBSim.java LIBRARY.mv.db OUT");
		}
		StringBuilder out = new StringBuilder();
		try (FunctionDatabase db = BSimClientFactory.buildClient(new BSimServerInfo(a[0]), false)) {
			if (!db.initialize()) {
				throw new IllegalStateException("the similarity library could not be opened: " + db.getLastError().message);
			}
			LSHVectorFactory vf = db.getLSHVectorFactory();
			GenSignatures gen = new GenSignatures(db.getInfo().trackcallgraph);
			try {
				gen.setVectorFactory(vf);
				gen.openProgram(currentProgram, null, null, null, null, null);
				FunctionManager fm = currentProgram.getFunctionManager();
				gen.scanFunctions(fm.getFunctions(true), fm.getFunctionCount(), monitor);
				DescriptionManager mine = gen.getDescriptionManager();
				String self = currentProgram.getExecutableMD5();
				ResponseExe count = new QueryExeCount().execute(db);
				out.append("Similarity library: ").append(count == null ? "?" : count.recordCount)
						.append(" programs analysed before. This file: ").append(mine.numFunctions())
						.append(" functions with code.\n");

				// the query: every function big enough to mean something, its closest functions in the library
				DescriptionManager ask = new DescriptionManager();
				ask.transferSettings(mine);
				int asked = 0;
				for (Iterator<FunctionDescription> it = mine.listAllFunctions(); it.hasNext();) {
					FunctionDescription f = it.next();
					SignatureRecord sig = f.getSignatureRecord();
					if (sig != null && !TOOLCHAIN.contains(f.getFunctionName()) &&
						vf.getSelfSignificance(sig.getLSHVector()) >= SELF_SIGNIFICANCE) {
						ask.transferFunction(f, true);
						asked++;
					}
				}
				Map<String, Exe> exes = new HashMap<>();
				List<String[]> pairs = new ArrayList<>(); // similarity, significance, line
				if (asked > 0) {
					QueryNearest q = new QueryNearest();
					q.manage = ask;
					q.max = PER_FUNCTION;
					q.thresh = SIMILARITY;
					q.signifthresh = 0.0;
					ResponseNearest r = q.execute(db);
					if (r == null) {
						throw new IllegalStateException("the comparison failed: " + db.getLastError().message);
					}
					for (SimilarityResult res : r.result) {
						FunctionDescription base = res.getBase();
						Set<String> counted = new HashSet<>();
						for (SimilarityNote n : res) {
							FunctionDescription m = n.getFunctionDescription();
							ExecutableRecord er = m.getExecutableRecord();
							if (er.getMd5().equals(self)) {
								continue; // this very file, added by an earlier run
							}
							Exe e = exes.computeIfAbsent(er.getMd5(), k -> new Exe());
							e.name = er.getNameExec();
							e.md5 = er.getMd5();
							if (counted.add(e.md5)) {
								e.matched++;
								e.significance += n.getSignificance();
							}
							e.best = Math.max(e.best, n.getSimilarity());
							pairs.add(new String[] { String.format("%.3f", n.getSimilarity()),
								String.format("%.1f", n.getSignificance()),
								"- " + plain(base.getFunctionName()) + String.format(" (0x%x)", base.getAddress()) + " ~ " +
									plain(er.getNameExec()) + ": " + plain(m.getFunctionName()) +
									String.format(" (0x%x)", m.getAddress()) });
						}
					}
				}
				List<Exe> ranked = new ArrayList<>(exes.values());
				ranked.sort((x, y) -> Double.compare(y.significance, x.significance) != 0
						? Double.compare(y.significance, x.significance)
						: x.md5.compareTo(y.md5));
				if (ranked.isEmpty()) {
					out.append("\nNo program in the library shares functions with this file (" + asked +
						" functions compared, similarity 0.7 or more).\n");
				}
				else {
					out.append("\nClosest programs (functions of this file with a match, best similarity, total significance):\n");
					for (int i = 0; i < Math.min(MAX_EXES, ranked.size()); i++) {
						Exe e = ranked.get(i);
						out.append(String.format("%d. %s (md5 %s): %d of %d functions, best %.3f, significance %.1f\n", i + 1,
							plain(e.name), plain(e.md5), e.matched, asked, e.best, e.significance));
					}
					if (ranked.size() > MAX_EXES) {
						out.append("and ").append(ranked.size() - MAX_EXES).append(" more\n");
					}
					pairs.sort((x, y) -> {
						int c = Double.compare(Double.parseDouble(y[1]), Double.parseDouble(x[1]));
						return c != 0 ? c : x[2].compareTo(y[2]);
					});
					out.append("\nClosest functions (this file's ~ the library's; similarity, significance):\n");
					for (int i = 0; i < Math.min(MAX_PAIRS, pairs.size()); i++) {
						String[] p = pairs.get(i);
						out.append(p[2]).append(": ").append(p[0]).append(", ").append(p[1]).append('\n');
					}
					if (pairs.size() > MAX_PAIRS) {
						out.append("and ").append(pairs.size() - MAX_PAIRS).append(" more pairs\n");
					}
				}

				// then this file joins the library, so the next sample can be compared with it
				if (mine.numFunctions() == 0) {
					out.append("\nNot added to the library: no functions with code.\n");
				}
				else {
					mine.listAllFunctions().forEachRemaining(f -> f.sortCallgraph());
					InsertRequest ins = new InsertRequest();
					ins.manage = mine;
					if (ins.execute(db) != null) {
						out.append("\nAdded to the library as ").append(plain(currentProgram.getName())).append(".\n");
					}
					else {
						BSimError err = db.getLastError();
						if (err != null && (err.category == ErrorCategory.Nonfatal || err.category == ErrorCategory.Format)) {
							out.append("\nAlready in the library (not added again).\n");
						}
						else {
							throw new IllegalStateException("adding it to the library failed: " + (err == null ? "?" : err.message));
						}
					}
				}
			}
			finally {
				gen.dispose();
			}
		}
		try (PrintWriter w = new PrintWriter(new File(a[1]), "UTF-8")) {
			w.print(out);
		}
	}
}
