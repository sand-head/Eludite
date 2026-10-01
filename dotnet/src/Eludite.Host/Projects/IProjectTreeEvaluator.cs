using Eludite.Host.Rpc;

namespace Eludite.Host.Projects;

/// <summary>Evaluates project files into Solution Explorer entries (<c>eludite/solution/tree</c>).</summary>
public interface IProjectTreeEvaluator
{
    /// <summary>One entry per path, in order. A project that does not evaluate gets <see cref="TreeProject.Error"/>.</summary>
    IReadOnlyList<TreeProject> Evaluate(IReadOnlyList<string> projectPaths, CancellationToken cancellationToken);
}
