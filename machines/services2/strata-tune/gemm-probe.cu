// Copied out of /tmp/probe/probe.cu on 2026-11-14; build it with gemm-probe.default.nix beside it.
// cuBLAS ceiling probe - is 1,060 TFLOP/s this card's bf16 limit, or the engine's limit?
//
// Why this exists (2026-11-14, services2).  The engine's prefill runs at a flat
// ~157-164 W on a card that is power-capped at 160 W (enforced, no controller
// running), which reads as "prompt speed is bought with watts and the meter is
// already at the ceiling".  But nobody had measured what the card does at that same
// power cap with NOTHING ELSE on it but a cuBLAS GEMM.  If a pure GEMM also lands at
// ~1,060 TFLOP/s, there is no headroom to chase and the only lever left is more
// accepted draft tokens.  If it lands much higher, then the engine is not at the
// ceiling - it is a bandwidth/latency mix, and the 160 W number was doing no work in
// the argument at all.
//
// Sized to run BESIDE the loaded engine (services2 keeps ~11.7 GiB of KV+activations
// on the card): N=2048 is 8 GiB of matrices, and the measurement (1.7e10 FLOP per
// GEMM, ~0.7 s) is long enough that a fixed overhead does not hide the rate.
//
// Built by /tmp/probe/default.nix; run with the helper /tmp/probe/run.sh, which samples
// nvidia-smi through the whole thing.

#include <cublas_v2.h>
#include <cuda_runtime.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <chrono>

#define CK(x)                                                                  \
    do {                                                                       \
        auto e_ = (x);                                                         \
        if (e_ != cudaSuccess) {                                               \
            std::fprintf(stderr, "cuda %s:%d %s\n", __FILE__, __LINE__,        \
                         cudaGetErrorString(e_));                              \
            return 1;                                                          \
        }                                                                      \
    } while (0)
// The status compared against a literal 0: with the enum name the test fired on every
// successful GemmEx on this CUDA (13.1) while the GEMMs demonstrably ran, and a probe
// that prints an error per call is worth less than one that prints none.
#define CB(x)                                                                  \
    do {                                                                       \
        auto e_ = (x);                                                         \
        if (e_ != 0) {                                     \
            std::fprintf(stderr, "cublas %s:%d code %d\n", __FILE__, __LINE__, \
                         (int) e_);                                            \
            return 1;                                                          \
        }                                                                      \
    } while (0)

int main(int argc, char** argv)
{
    const int n = argc > 1 ? std::atoi(argv[1]) : 4096;
    const int iters = argc > 2 ? std::atoi(argv[2]) : 1;
    const int seconds = argc > 3 ? std::atoi(argv[3]) : 20;   // keep it warm that long
    int dev = 0;
    cudaDeviceProp p{};
    CK(cudaGetDevice(&dev));
    CK(cudaGetDeviceProperties(&p, dev));
    int clk = 0, memclk = 0, l2 = 0;
    CK(cudaDeviceGetAttribute(&clk, cudaDevAttrClockRate, dev));
    CK(cudaDeviceGetAttribute(&memclk, cudaDevAttrMemoryClockRate, dev));
    CK(cudaDeviceGetAttribute(&l2, cudaDevAttrL2CacheSize, dev));
    std::printf("gpu: %s  sm_%d%d  multiprocessors=%d  clock=%d kHz  memclock=%d kHz  l2=%d KiB\n",
                p.name, p.major, p.minor, p.multiProcessorCount, clk, memclk, l2 / 1024);

    __half *A, *B, *C, *D;
    const size_t bytes = (size_t) n * n * sizeof(__half);
    CK(cudaMalloc(&A, bytes));
    CK(cudaMalloc(&B, bytes));
    CK(cudaMalloc(&C, bytes));
    CK(cudaMalloc(&D, bytes));
    CK(cudaMemset(A, 0x3c, bytes));   // junk bits: the rate of a real GEMM, not of zeros
    CK(cudaMemset(B, 0x3c, bytes));
    CK(cudaMemset(C, 0, bytes));
    CK(cudaMemset(D, 0, bytes));

    cublasHandle_t h;
    CB(cublasCreate(&h));
    CB(cublasSetMathMode(h, CUBLAS_DEFAULT_MATH));
    const float alpha = 1.0f, beta = 0.0f;
    auto gemm = [&] {
        CB(cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, n, n, n, &alpha, A, CUDA_R_16BF, n, B,
                        CUDA_R_16BF, n, &beta, C, CUDA_R_16BF, n, CUBLAS_COMPUTE_32F,
                        CUBLAS_GEMM_DEFAULT));
        CB(cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, n, n, n, &alpha, A, CUDA_R_16BF, n, B,
                        CUDA_R_16BF, n, &beta, D, CUDA_R_16BF, n, CUBLAS_COMPUTE_32F,
                        CUBLAS_GEMM_DEFAULT));
    };
    gemm();                       // prime the algorithm-selection cache
    CK(cudaDeviceSynchronize());

    cudaEvent_t e0, e1;
    CK(cudaEventCreate(&e0));
    CK(cudaEventCreate(&e1));
    const double flop_per_round = 2.0 * 2.0 * (double) n * n * n * iters;   // two GEMMs
    double best = 0.0, sum = 0.0, wallsum = 0.0;
    const auto tw0 = std::chrono::steady_clock::now();
    int rounds = 0;
    cudaEventRecord(e0);
    for (;;) {
        for (int i = 0; i < iters; ++i)
            gemm();
        CK(cudaEventRecord(e1));
        CK(cudaEventSynchronize(e1));
        float ms = 0.0f;
        CK(cudaEventElapsedTime(&ms, e0, e1));
        const double tf = flop_per_round / (ms / 1000.0) / 1e12;
        const double wall = std::chrono::duration<double>(std::chrono::steady_clock::now() - tw0).count();
        const double tfw = flop_per_round * rounds / wall / 1e12;   // independent of CUDA events
        wallsum = tfw;
        best = best < tf ? tf : best;
        sum += tf;
        ++rounds;
        cudaEventRecord(e0);
        if (rounds * ms / 1000.0 > seconds)
            break;
    }
    std::printf("bf16 n=%d  rounds=%d  best=%.0f TFLOP/s  mean=%.0f TFLOP/s  wall=%.0f TFLOP/s (event vs clock)\n", n, rounds, best,
                sum / rounds, wallsum);
    std::fflush(stdout);
    return 0;
}
