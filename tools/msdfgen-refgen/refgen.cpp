// Reference fields for crates/slam-text/tests/data/msdfgen-corrected.txt and
// msdfgen-simple.txt (see README.md). msdfgen v1.13 pipeline with overlap support
// and scanline pass, or ("simple:<name>") orientContours and neither, as the
// utility does after geometry preprocessing; then EDGE_PRIORITY / DO_NOT_CHECK_DISTANCE.
#include <cstdio>
#include <string>
#include <vector>
#include "msdfgen.h"
using namespace msdfgen;

static Point2 p(double x, double y) { return Point2(x, y); }

static Contour polygon(std::vector<Point2> pts) {
    Contour c;
    size_t n = pts.size();
    for (size_t i = 0; i < n; ++i) c.addEdge(EdgeHolder(pts[i], pts[(i + 1) % n]));
    return c;
}
static Contour square(double x0, double y0, double x1, double y1) {
    return polygon({p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)});
}
static Contour circle(Point2 c, double r, bool ccw) {
    double k = 0.5522847498307936 * r;
    Point2 pts[4] = {p(c.x + r, c.y), p(c.x, c.y + r), p(c.x - r, c.y), p(c.x, c.y - r)};
    Vector2 t[4] = {Vector2(0, k), Vector2(-k, 0), Vector2(0, -k), Vector2(k, 0)};
    Contour contour;
    for (int i = 0; i < 4; ++i) {
        int j = (i + 1) % 4;
        contour.addEdge(EdgeHolder(pts[i], pts[i] + t[i], pts[j] - t[j], pts[j]));
    }
    if (!ccw) contour.reverse();
    return contour;
}
typedef EdgeHolder E;
static Contour edges(std::vector<EdgeHolder> e) {
    Contour c;
    for (auto &x : e) c.addEdge(x);
    return c;
}

static Shape shape(const std::string &n) {
    Shape s;
    auto add = [&](Contour c) { s.addContour(c); };
    if (n == "square") add(square(2, 2, 8, 8));
    else if (n == "square_cw") add(polygon({p(2, 2), p(2, 8), p(8, 8), p(8, 2)}));
    else if (n == "overlap") { add(square(2, 2, 8, 8)); add(square(5, 4, 11, 10)); }
    else if (n == "ring") { add(circle(p(7, 7), 5, true)); add(circle(p(7, 7), 2.5, false)); }
    else if (n == "ring_inverted") { add(circle(p(7, 7), 5, false)); add(circle(p(7, 7), 2.5, true)); }
    else if (n == "line_quad") add(edges({E(p(2, 2), p(10, 2)), E(p(10, 2), p(10, 9), p(2, 2))}));
    else if (n == "cusp_quad") add(edges({E(p(2, 2), p(6, 5), p(10, 5)), E(p(10, 5), p(6, 5), p(2, 8)), E(p(2, 8), p(2, 2))}));
    else if (n == "cusp_both_degenerate") add(edges({E(p(2, 2), p(4, 6), p(10, 5), p(10, 5)), E(p(10, 5), p(10, 5), p(4, 6), p(3, 3)), E(p(3, 3), p(2, 2))}));
    else if (n == "cusp_before_degenerate") add(edges({E(p(2, 2), p(4, 6), p(10, 5), p(10, 5)), E(p(10, 5), p(4, 6), p(3, 9)), E(p(3, 9), p(2, 2))}));
    else if (n == "cusp_after_degenerate") add(edges({E(p(2, 8), p(4, 6), p(10, 5)), E(p(10, 5), p(10, 5), p(4, 6), p(3, 2)), E(p(3, 2), p(2, 8))}));
    else if (n == "cusp_line_cubic") add(edges({E(p(2, 5), p(11, 5)), E(p(11, 5), p(5, 5), p(6, 10), p(2, 9)), E(p(2, 9), p(2, 5))}));
    else if (n == "single_cubic") add(edges({E(p(2, 2), p(14, 6), p(6, 14), p(2, 2))}));
    else if (n == "lens0") add(edges({E(p(2, 2), p(12, 2), p(12, 8)), E(p(12, 8), p(12, 14), p(2, 2))}));
    else if (n == "lens1") add(edges({E(p(12, 8), p(12, 14), p(2, 2)), E(p(2, 2), p(12, 2), p(12, 8))}));
    else if (n == "half_square") add(square(2.5, 2.5, 7.5, 7.5));
    else if (n == "inverted_half") add(polygon({p(2.5, 2.5), p(2.5, 7.5), p(7.5, 7.5), p(7.5, 2.5)}));
    else if (n == "s_cubic") add(edges({E(p(1, 5), p(5, 12), p(9, -2), p(13, 5)), E(p(13, 5), p(13, 1)), E(p(13, 1), p(1, 1)), E(p(1, 1), p(1, 5))}));
    else if (n == "s_cubic_half") add(edges({E(p(1, 5.5), p(5, 12.5), p(9, -1.5), p(13, 5.5)), E(p(13, 5.5), p(13, 1.5)), E(p(13, 1.5), p(1, 1.5)), E(p(1, 1.5), p(1, 5.5))}));
    else if (n == "self_overlap_star") add(polygon({p(7, 1), p(10.5, 12), p(1.5, 5), p(12.5, 5), p(3.5, 12)}));
    else if (n == "horizontal_quad_tangent") {
        add(edges({E(p(2, 2), p(7, 12), p(12, 2)), E(p(12, 2), p(2, 2))}));
        Contour bar = square(3, 7.5, 11, 9.5);
        bar.edges[0] = E(p(3, 7.5), p(7, 7.5), p(11, 7.5));
        add(bar);
    } else { fprintf(stderr, "unknown %s\n", n.c_str()); exit(1); }
    return s;
}

int main(int argc, char **argv) {
    for (int i = 1; i + 2 < argc; i += 3) {
        std::string name = argv[i];
        int w = atoi(argv[i + 1]), h = atoi(argv[i + 2]);
        bool simple = name.compare(0, 7, "simple:") == 0;
        if (simple) name = name.substr(7);
        // "<shape>@scaled": projection scale (1.5, 2), translate (0.5, -0.25), range 3.
        bool scaled = name.size() > 7 && name.substr(name.size() - 7) == "@scaled";
        Shape s = shape(scaled ? name.substr(0, name.size() - 7) : name);
        if (simple) s.orientContours();
        s.normalize();
        edgeColoringSimple(s, 3.0, 0);
        SDFTransformation tr = scaled
            ? SDFTransformation(Projection(Vector2(1.5, 2), Vector2(0.5, -0.25)), Range(3.0))
            : SDFTransformation(Projection(Vector2(1), Vector2(0)), Range(4.0));
        Bitmap<float, 3> msdf(w, h);
        MSDFGeneratorConfig gen(!simple, ErrorCorrectionConfig(ErrorCorrectionConfig::DISABLED));
        generateMSDF(msdf, s, tr, gen);
        // Before error correction, so the test can count changed pixels.
        std::vector<float> before;
        if (!simple) distanceSignCorrection(msdf, s, tr, .5f, FILL_NONZERO);
        for (int y = 0; y < h; ++y) for (int x = 0; x < w; ++x) for (int c = 0; c < 3; ++c) before.push_back(msdf(x, y)[c]);
        MSDFGeneratorConfig post(!simple, ErrorCorrectionConfig(ErrorCorrectionConfig::EDGE_PRIORITY, ErrorCorrectionConfig::DO_NOT_CHECK_DISTANCE));
        msdfErrorCorrection(msdf, s, tr, post);
        int changed = 0;
        printf("%s %s %d %d", simple ? "simple" : "corrected", name.c_str(), w, h);
        for (int y = 0; y < h; ++y) for (int x = 0; x < w; ++x) {
            bool ch = false;
            for (int c = 0; c < 3; ++c) {
                float v = msdf(x, y)[c];
                ch |= v != before[(y * w + x) * 3 + c];
                printf(" %.9g", v);
            }
            changed += ch;
        }
        printf("\n");
        fprintf(stderr, "%s: %d pixels corrected\n", name.c_str(), changed);
    }
}
